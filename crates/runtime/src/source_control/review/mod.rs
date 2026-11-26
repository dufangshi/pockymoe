//! Pull request review across hosts: one normalized model for lists, detail,
//! files and review threads, so the Web, the CLI and agents read the same shape
//! whatever the host. Host-specific requests live in one module per host; local
//! state (drafts, viewed files, guides, saved views) lives in `store`.
use super::hosts::{Host, HostKind, Repository};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub mod credits;
mod forgejo;
mod github;
mod gitlab;
pub mod guide;
mod service;
pub mod stack;
pub mod store;
#[cfg(test)]
mod tests;

pub use service::ReviewCaller;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub login: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}
fn person(login: Option<&str>, avatar: Option<&str>) -> Option<Person> {
    login.filter(|l| !l.is_empty()).map(|login| Person {
        login: login.into(),
        avatar_url: avatar.filter(|a| a.starts_with("https://")).map(Into::into),
    })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// One row of the Pull requests page.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PullSummary {
    pub url: String,
    pub number: u64,
    pub title: String,
    /// `open`, `closed` or `merged`.
    pub state: String,
    pub draft: bool,
    pub author: Option<Person>,
    pub repository: Repository,
    pub head_ref: String,
    pub base_ref: String,
    pub head_sha: String,
    pub created_at: String,
    pub updated_at: String,
    /// `passed`, `failed`, `pending`, `none`, or null when the host list does not say.
    pub checks: Option<String>,
    /// `approved`, `changesRequested`, `reviewRequired`, or null.
    pub review_decision: Option<String>,
    pub requested_reviewers: Vec<String>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub changed_files: Option<u64>,
    pub comments: Option<u64>,
    pub labels: Vec<Label>,
    /// Where the PR sits in a stack (host-native or inferred from its base).
    #[serde(default)]
    pub stack: Option<StackRef>,
}

/// A PR's place in a stack: `position` 1 is the layer on the base branch.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StackRef {
    pub id: String,
    pub position: u64,
    pub size: u64,
    /// The host keeps the stack (GitHub stacks); otherwise it is inferred.
    pub native: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Reviewer {
    pub login: String,
    /// `requested`, `approved`, `changesRequested`, `commented`, `dismissed`.
    pub state: String,
    #[serde(default)]
    pub team: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEntry {
    /// `comment` (conversation) or `review` (a submitted review).
    pub kind: String,
    pub id: String,
    pub author: Option<Person>,
    pub body: String,
    pub created_at: String,
    pub url: Option<String>,
    /// Reviews only: `approved`, `changesRequested`, `commented`, `dismissed`.
    pub state: Option<String>,
    pub commit: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadComment {
    pub id: String,
    pub author: Option<Person>,
    pub body: String,
    pub created_at: String,
    pub url: Option<String>,
}

/// An inline discussion on a file or a line range.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThread {
    pub id: String,
    pub path: String,
    pub line: Option<u64>,
    pub start_line: Option<u64>,
    /// `left` (old file) or `right` (new file).
    pub side: String,
    /// `line` or `file`.
    pub subject: String,
    pub resolved: bool,
    pub outdated: bool,
    pub can_resolve: bool,
    pub can_reply: bool,
    pub comments: Vec<ThreadComment>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub login: Option<String>,
    pub date: String,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: String,
    pub name: String,
    /// `passed`, `failed`, `pending`, `skipped`, `neutral`, `cancelled`.
    pub state: String,
    pub url: Option<String>,
    pub description: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    /// Opaque token for `rerun` when the host can rerun this check's failed jobs.
    pub rerun: Option<String>,
}

/// What the host lets this review surface do. The UI hides what is false and
/// the service refuses it, so a missing host feature is never guessed.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub submit: Vec<String>,
    pub line_comments: bool,
    pub file_comments: bool,
    pub reply: bool,
    pub resolve: bool,
    pub viewed_native: bool,
    pub reviewers: bool,
    pub convert_to_draft: bool,
    pub ready: bool,
    pub close: bool,
    pub reopen: bool,
    pub auto_merge: bool,
    pub merge: bool,
    pub revert: bool,
    pub rerun: bool,
    pub remove_credits: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PullDetail {
    #[serde(flatten)]
    pub summary: PullSummary,
    pub body: String,
    pub node_id: Option<String>,
    pub base_sha: Option<String>,
    pub mergeable: Option<bool>,
    /// Host merge state in our words: `clean`, `conflicts`, `blocked`, `behind`, `unstable`, `unknown`.
    pub merge_state: Option<String>,
    pub auto_merge: Option<String>,
    pub reviewers: Vec<Reviewer>,
    pub timeline: Vec<TimelineEntry>,
    pub threads: Vec<ReviewThread>,
    pub commits: Vec<Commit>,
    pub check_runs: Vec<Check>,
    pub viewer: Option<String>,
    pub merge_methods: Vec<String>,
    pub capabilities: Capabilities,
    /// Host data later writes need (GitLab diff refs, a fork's clone URL).
    #[serde(skip)]
    pub extra: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PullFile {
    pub path: String,
    pub old_path: Option<String>,
    /// `added`, `modified`, `removed`, `renamed`, `copied`.
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    /// The file as a unified diff with a `diff --git` header, or null when the
    /// host withheld it (binary or too large).
    pub patch: Option<String>,
    pub binary: bool,
    /// Changes when the file's diff changes; marks a viewed file as changed.
    pub digest: String,
    /// `unviewed`, `viewed` or `changed` (viewed before the latest push).
    pub viewed: String,
}

/// A draft comment kept on the Supervisor until its review is submitted.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub id: String,
    /// `line`, `file` or `summary` (the review's own text).
    pub kind: String,
    pub path: Option<String>,
    pub side: Option<String>,
    pub line: Option<u64>,
    pub start_line: Option<u64>,
    pub body: String,
    /// The PR head the comment was written against.
    pub head_sha: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub(crate) fn pr_key(repo: &Repository, number: u64) -> String {
    format!("{}/{}/{}#{number}", repo.host, repo.owner, repo.name)
}

/// The three review outcomes, in the words every host maps from.
pub(crate) fn review_event(value: &str) -> Result<&'static str> {
    Ok(match value {
        "comment" => "comment",
        "approve" => "approve",
        "requestChanges" => "requestChanges",
        _ => bail!("Review event must be comment, approve or requestChanges"),
    })
}

/// `diff --git` file text around a host's hunk-only patch, so the Web's patch
/// parser and the turn diff viewer read host files like local Git output.
pub(crate) fn unified_patch(
    path: &str,
    old_path: Option<&str>,
    status: &str,
    hunks: Option<&str>,
) -> Option<String> {
    let old = old_path.unwrap_or(path);
    let mut text = format!("diff --git a/{old} b/{path}\n");
    match status {
        "added" => text.push_str("new file mode 100644\n"),
        "removed" => text.push_str("deleted file mode 100644\n"),
        "renamed" if old != path => {
            text.push_str(&format!("rename from {old}\nrename to {path}\n"))
        }
        _ => {}
    }
    let hunks = hunks.filter(|h| !h.is_empty());
    match hunks {
        Some(hunks) => {
            let from = if status == "added" {
                "/dev/null".into()
            } else {
                format!("a/{old}")
            };
            let to = if status == "removed" {
                "/dev/null".into()
            } else {
                format!("b/{path}")
            };
            text.push_str(&format!("--- {from}\n+++ {to}\n{hunks}"));
            if !hunks.ends_with('\n') {
                text.push('\n');
            }
            Some(text)
        }
        None if status == "renamed" && old != path => Some(text),
        None => None,
    }
}

pub(crate) fn digest(text: &str) -> String {
    use sha2::Digest;
    let hash = sha2::Sha256::digest(text.as_bytes());
    hash.iter().take(12).map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn file_status(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "added" | "add" | "new" => "added",
        "removed" | "deleted" | "delete" => "removed",
        "renamed" | "rename" => "renamed",
        "copied" | "copy" => "copied",
        _ => "modified",
    }
    .into()
}

/// Truncate host text the UI shows inline (bodies stay whole up to this bound).
pub(crate) fn bounded(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// A GraphQL call: host errors become one readable error, never a raw body.
pub(crate) async fn graphql(host: &dyn Host, query: &str, variables: Value) -> Result<Value> {
    let value = host
        .request(
            "POST",
            "graphql",
            Some(json!({"query":query,"variables":variables})),
        )
        .await?;
    if let Some(errors) = value["errors"].as_array().filter(|e| !e.is_empty()) {
        let message = errors[0]["message"]
            .as_str()
            .unwrap_or("GraphQL request failed");
        bail!("Host refused the request: {}", bounded(message, 300));
    }
    Ok(value["data"].clone())
}

pub(crate) fn str_of(value: &Value) -> String {
    value.as_str().unwrap_or("").into()
}
pub(crate) fn opt_str(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(Into::into)
}

/// The input for `submit`: an outcome, the review's own text and the drafts.
pub struct Submission<'a> {
    pub event: &'static str,
    pub body: &'a str,
    pub comments: &'a [Draft],
    pub head: &'a str,
}

/// Check a submission against what the host can post before anything is sent.
pub(crate) fn validate_submission(caps: &Capabilities, input: &Submission) -> Result<()> {
    ensure!(
        caps.submit.iter().any(|e| e == input.event),
        "This host cannot submit a review with that outcome"
    );
    if input.event != "approve" {
        ensure!(
            !input.body.trim().is_empty() || !input.comments.is_empty(),
            "Write a comment or add line comments before submitting"
        );
    }
    for draft in input.comments {
        ensure!(!draft.body.trim().is_empty(), "A draft comment is empty");
        match draft.kind.as_str() {
            "line" => {
                ensure!(caps.line_comments, "This host cannot post line comments");
                ensure!(
                    draft.path.is_some() && draft.line.is_some_and(|l| l > 0),
                    "A line comment needs a path and line"
                );
            }
            "file" => ensure!(draft.path.is_some(), "A file comment needs a path"),
            _ => bail!("Only line and file drafts are submitted"),
        }
    }
    Ok(())
}

/// File comments on hosts without them become a line in the review body, so
/// nothing the reviewer wrote is dropped silently.
pub(crate) fn fold_file_comments(caps: &Capabilities, body: &str, drafts: &[Draft]) -> String {
    if caps.file_comments {
        return body.into();
    }
    let mut text = body.trim_end().to_owned();
    for draft in drafts.iter().filter(|d| d.kind == "file") {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&format!(
            "**{}**\n\n{}",
            draft.path.as_deref().unwrap_or(""),
            draft.body.trim()
        ));
    }
    text
}

pub async fn list(host: &dyn Host, repo: &Repository, state: &str) -> Result<Vec<PullSummary>> {
    ensure!(
        matches!(state, "open" | "closed" | "merged" | "all"),
        "State must be open, closed, merged or all"
    );
    match repo.kind {
        HostKind::Github => github::list(host, repo, state).await,
        HostKind::Gitlab => gitlab::list(host, repo, state).await,
        HostKind::Forgejo => forgejo::list(host, repo, state).await,
    }
}

pub async fn detail(host: &dyn Host, repo: &Repository, number: u64) -> Result<PullDetail> {
    ensure!(number > 0, "PR number required");
    match repo.kind {
        HostKind::Github => github::detail(host, repo, number).await,
        HostKind::Gitlab => gitlab::detail(host, repo, number).await,
        HostKind::Forgejo => forgejo::detail(host, repo, number).await,
    }
}

/// Only the head (cheap): used to notice new commits during a review.
pub async fn head(host: &dyn Host, repo: &Repository, number: u64) -> Result<String> {
    let op = host.operation("show", false, Some(number), &json!({}))?;
    let value = host.request(&op.method, &op.path, None).await?;
    let head = super::hosts::head_oid(repo.kind, &value);
    ensure!(!head.is_empty(), "PR head unavailable");
    Ok(head)
}

/// The PR's changed files with viewed state: GitHub's own, else the store's
/// (applied by the caller, which owns the database).
pub async fn files(host: &dyn Host, repo: &Repository, number: u64) -> Result<Vec<PullFile>> {
    ensure!(number > 0, "PR number required");
    match repo.kind {
        HostKind::Github => {
            let (files, viewed) = tokio::join!(
                github::files(host, repo, number),
                github::viewed(host, repo, number)
            );
            let mut files = files?;
            let viewed = viewed?;
            for file in &mut files {
                if let Some((_, state)) = viewed.iter().find(|(p, _)| p == &file.path) {
                    file.viewed = state.clone();
                }
            }
            Ok(files)
        }
        HostKind::Gitlab => gitlab::files(host, repo, number).await,
        HostKind::Forgejo => forgejo::files(host, repo, number).await,
    }
}

/// Head guard shared by submit and merge: a review or merge of a commit the
/// reviewer has not loaded is refused, never silently applied to the new head.
pub fn guard_head(expected: &str, current: &str) -> Result<()> {
    ensure!(
        !expected.is_empty() && expected == current,
        "conflict: PR head moved; load new commits before submitting or merging"
    );
    Ok(())
}

pub async fn submit(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    input: &Submission<'_>,
) -> Result<Value> {
    validate_submission(&detail.capabilities, input)?;
    guard_head(input.head, &detail.summary.head_sha)?;
    match repo.kind {
        HostKind::Github => github::submit(host, repo, detail, input).await,
        HostKind::Gitlab => gitlab::submit(host, repo, detail, input).await,
        HostKind::Forgejo => forgejo::submit(host, repo, detail, input).await,
    }
}

pub async fn reply(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    thread: &str,
    body: &str,
) -> Result<Value> {
    ensure!(
        detail.capabilities.reply,
        "This host cannot reply to review threads here"
    );
    ensure!(!body.trim().is_empty(), "Reply required");
    let target = detail
        .threads
        .iter()
        .find(|t| t.id == thread)
        .context("Review thread not found; reload the PR")?;
    match repo.kind {
        HostKind::Github => github::reply(host, thread, body).await,
        HostKind::Gitlab => gitlab::reply(host, repo, detail.summary.number, thread, body).await,
        HostKind::Forgejo => forgejo::reply(host, repo, detail, target, body).await,
    }
}

pub async fn resolve(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    thread: &str,
    resolved: bool,
) -> Result<Value> {
    ensure!(
        detail.capabilities.resolve,
        "This host cannot resolve review threads here"
    );
    let target = detail
        .threads
        .iter()
        .find(|t| t.id == thread)
        .context("Review thread not found; reload the PR")?;
    ensure!(target.can_resolve, "You cannot resolve this thread");
    match repo.kind {
        HostKind::Github => github::resolve(host, thread, resolved).await,
        HostKind::Gitlab => {
            gitlab::resolve(host, repo, detail.summary.number, thread, resolved).await
        }
        HostKind::Forgejo => bail!("Forgejo has no API to resolve conversations"),
    }
}

pub async fn reviewers(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    add: &[String],
    remove: &[String],
) -> Result<Value> {
    ensure!(
        detail.capabilities.reviewers,
        "This host cannot request reviewers here"
    );
    ensure!(!add.is_empty() || !remove.is_empty(), "Name a reviewer");
    for login in add.iter().chain(remove) {
        ensure!(
            !login.is_empty()
                && login.len() <= 100
                && login
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/')),
            "Invalid reviewer name"
        );
    }
    match repo.kind {
        HostKind::Github => github::reviewers(host, repo, detail.summary.number, add, remove).await,
        HostKind::Gitlab => gitlab::reviewers(host, repo, detail, add, remove).await,
        HostKind::Forgejo => {
            forgejo::reviewers(host, repo, detail.summary.number, add, remove).await
        }
    }
}

/// `close`, `reopen`, `draft` (convert to draft) or `ready`.
pub async fn set_state(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    state: &str,
) -> Result<Value> {
    let caps = &detail.capabilities;
    let allowed = match state {
        "close" => caps.close && detail.summary.state == "open",
        "reopen" => caps.reopen && detail.summary.state == "closed",
        "draft" => caps.convert_to_draft && !detail.summary.draft && detail.summary.state == "open",
        "ready" => caps.ready && detail.summary.draft && detail.summary.state == "open",
        _ => bail!("State change must be close, reopen, draft or ready"),
    };
    ensure!(allowed, "This PR cannot change to that state here");
    match repo.kind {
        HostKind::Github => github::set_state(host, repo, detail, state).await,
        HostKind::Gitlab => gitlab::set_state(host, repo, detail, state).await,
        HostKind::Forgejo => forgejo::set_state(host, repo, detail, state).await,
    }
}

pub async fn rerun(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    token: &str,
) -> Result<Value> {
    ensure!(
        detail.capabilities.rerun,
        "This host cannot rerun checks here"
    );
    ensure!(
        detail
            .check_runs
            .iter()
            .any(|c| c.rerun.as_deref() == Some(token)),
        "That check cannot be rerun; reload the PR"
    );
    match repo.kind {
        HostKind::Github => github::rerun(host, repo, token).await,
        HostKind::Gitlab => gitlab::rerun(host, repo, token).await,
        _ => bail!("This host cannot rerun checks here"),
    }
}

pub async fn revert(host: &dyn Host, repo: &Repository, detail: &PullDetail) -> Result<Value> {
    ensure!(
        detail.capabilities.revert && detail.summary.state == "merged",
        "Only a merged GitHub PR can be reverted here"
    );
    match repo.kind {
        HostKind::Github => github::revert(host, detail).await,
        _ => bail!("Revert PRs are available on GitHub"),
    }
}

pub struct MergeInput<'a> {
    pub method: &'a str,
    pub expected_head: &'a str,
    pub remove_credits: bool,
}

pub async fn merge(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    input: &MergeInput<'_>,
) -> Result<Value> {
    ensure!(detail.capabilities.merge, "This host cannot merge here");
    ensure!(detail.summary.state == "open", "PR is not open");
    ensure!(
        detail.merge_methods.iter().any(|m| m == input.method),
        "That merge method is not allowed for this repository"
    );
    guard_head(input.expected_head, &detail.summary.head_sha)?;
    let message = if input.remove_credits {
        ensure!(
            detail.capabilities.remove_credits && input.method != "rebase",
            "Agent credits can be removed from squash or merge commits only"
        );
        Some(merge_message(host, repo, detail, input.method).await?)
    } else {
        None
    };
    match repo.kind {
        HostKind::Github => github::merge(host, repo, detail, input, message).await,
        HostKind::Gitlab => gitlab::merge(host, repo, detail, input, message).await,
        HostKind::Forgejo => forgejo::merge(host, repo, detail, input, message).await,
    }
}

pub async fn auto_merge(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    input: &MergeInput<'_>,
    enable: bool,
) -> Result<Value> {
    ensure!(
        detail.capabilities.auto_merge,
        "This host cannot auto-merge here"
    );
    if enable {
        guard_head(input.expected_head, &detail.summary.head_sha)?;
    }
    let message = if enable && input.remove_credits && input.method != "rebase" {
        Some(merge_message(host, repo, detail, input.method).await?)
    } else {
        None
    };
    match repo.kind {
        HostKind::Github => github::auto_merge(host, detail, input, enable, message).await,
        HostKind::Gitlab => gitlab::auto_merge(host, repo, detail, input, enable).await,
        HostKind::Forgejo => forgejo::auto_merge(host, repo, detail, input, enable).await,
    }
}

/// Native viewed state (GitHub). Other hosts keep it in `store`.
pub async fn set_viewed_native(
    host: &dyn Host,
    detail: &PullDetail,
    path: &str,
    viewed: bool,
) -> Result<()> {
    ensure!(
        detail.capabilities.viewed_native,
        "Viewed state is stored on this device"
    );
    github::set_viewed(host, detail, path, viewed).await
}

/// The host's own default merge message for this method, without agent credits.
/// Hosts that cannot say build GitHub-style text from the PR's commits.
pub async fn merge_message(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    method: &str,
) -> Result<(String, String)> {
    let (title, body) = match repo.kind {
        HostKind::Github => github::merge_message(host, repo, detail, method).await?,
        HostKind::Gitlab => gitlab::merge_message(host, repo, detail, method).await?,
        _ => built_merge_message(detail, method),
    };
    let body = credits::remove_agent_credits(&body);
    Ok((title, body))
}

pub(crate) fn built_merge_message(detail: &PullDetail, method: &str) -> (String, String) {
    let s = &detail.summary;
    if method == "squash" {
        let body = if detail.commits.len() == 1 {
            detail.commits[0]
                .message
                .split_once('\n')
                .map(|(_, rest)| rest.trim_start_matches('\n').to_owned())
                .unwrap_or_default()
        } else {
            detail
                .commits
                .iter()
                .map(|c| format!("* {}", c.message.trim()))
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        (format!("{} (#{})", s.title, s.number), body)
    } else {
        (
            format!("Merge pull request #{} from {}", s.number, s.head_ref),
            s.title.clone(),
        )
    }
}

fn diff_path(text: &str) -> String {
    let text = text.trim();
    let text = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(text);
    text.strip_prefix("a/")
        .or_else(|| text.strip_prefix("b/"))
        .unwrap_or(text)
        .to_owned()
}

/// Files of a whole `git diff`, as hosts that serve a raw diff return it. The
/// new-side blob id (from `index a..b`) is the viewed-state revision when known.
pub(crate) fn files_from_diff(text: &str) -> Vec<PullFile> {
    let mut files = Vec::new();
    for chunk in text.split("\ndiff --git ").enumerate().map(|(i, c)| {
        if i == 0 {
            c.strip_prefix("diff --git ").unwrap_or(c).to_owned()
        } else {
            c.to_owned()
        }
    }) {
        if chunk.trim().is_empty() || !chunk.contains(' ') {
            continue;
        }
        let header = chunk.lines().next().unwrap_or("");
        let (mut old, mut path) = match header.split_once(" b/") {
            Some((a, b)) => (diff_path(a), b.to_owned()),
            None => (header.into(), header.into()),
        };
        let mut status = "modified";
        let mut blob = String::new();
        let (mut additions, mut deletions, mut binary, mut in_hunk) = (0, 0, false, false);
        for line in chunk.lines().skip(1) {
            if line.starts_with("@@") {
                in_hunk = true;
                continue;
            }
            if in_hunk {
                if line.starts_with('+') {
                    additions += 1;
                } else if line.starts_with('-') {
                    deletions += 1;
                }
                continue;
            }
            if line.starts_with("new file mode") {
                status = "added";
            } else if line.starts_with("deleted file mode") {
                status = "removed";
            } else if let Some(from) = line.strip_prefix("rename from ") {
                status = "renamed";
                old = from.into();
            } else if let Some(to) = line.strip_prefix("rename to ") {
                path = to.into();
            } else if let Some(index) = line.strip_prefix("index ") {
                blob = index
                    .split(' ')
                    .next()
                    .and_then(|r| r.split_once(".."))
                    .map(|(_, b)| b.to_owned())
                    .unwrap_or_default();
            } else if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
                binary = true;
            } else if let Some(to) = line.strip_prefix("+++ ") {
                if to != "/dev/null" {
                    path = diff_path(to);
                }
            }
        }
        let patch = format!("diff --git {}\n", chunk.trim_end_matches('\n'));
        files.push(PullFile {
            digest: if blob.chars().any(|c| c != '0') && !blob.is_empty() {
                blob
            } else {
                digest(&patch)
            },
            old_path: (old != path).then_some(old),
            status: status.into(),
            additions,
            deletions,
            patch: Some(patch),
            binary,
            viewed: "unviewed".into(),
            path,
        });
    }
    files
}
