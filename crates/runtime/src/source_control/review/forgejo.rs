//! Forgejo and Gitea over REST. Their review API mirrors GitHub's closely;
//! there is no API to resolve a conversation or reply to a comment directly, so
//! a reply is a one-comment review on the same line (the host shows it in the
//! same conversation), and resolving is left to the host's page.
use super::*;
use crate::source_control::hosts::Host;
use futures_util::{stream, StreamExt};

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
fn root(repo: &Repository) -> String {
    format!("repos/{}/{}", enc(&repo.owner), enc(&repo.name))
}
fn state(p: &Value) -> String {
    if p["merged"] == true {
        "merged".into()
    } else if p["state"] == "closed" {
        "closed".into()
    } else {
        "open".into()
    }
}
fn draft_title(title: &str) -> bool {
    let lower = title.to_lowercase();
    lower.starts_with("wip:") || lower.starts_with("[wip]") || lower.starts_with("draft:")
}
fn combined(value: &Value) -> Option<String> {
    if value["total_count"].as_u64() == Some(0)
        || value["statuses"].as_array().is_some_and(|s| s.is_empty())
    {
        return Some("none".into());
    }
    Some(
        match value["state"].as_str()? {
            "success" => "passed",
            "failure" | "error" => "failed",
            "pending" => "pending",
            "warning" => "passed",
            _ => "none",
        }
        .into(),
    )
}

pub(super) fn summary(repo: &Repository, p: &Value) -> PullSummary {
    let title = str_of(&p["title"]);
    PullSummary {
        stack: None,
        url: str_of(&p["html_url"]),
        number: p["number"].as_u64().unwrap_or(0),
        draft: p["draft"] == true || draft_title(&title),
        title,
        state: state(p),
        author: person(
            p["user"]["login"].as_str(),
            p["user"]["avatar_url"].as_str(),
        ),
        repository: repo.clone(),
        head_ref: str_of(&p["head"]["ref"]),
        base_ref: str_of(&p["base"]["ref"]),
        head_sha: str_of(&p["head"]["sha"]),
        created_at: str_of(&p["created_at"]),
        updated_at: str_of(&p["updated_at"]),
        checks: None,
        review_decision: None,
        requested_reviewers: p["requested_reviewers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| opt_str(&r["login"]))
            .collect(),
        additions: p["additions"].as_u64(),
        deletions: p["deletions"].as_u64(),
        changed_files: p["changed_files"].as_u64(),
        comments: p["comments"].as_u64(),
        labels: p["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|l| Label {
                name: str_of(&l["name"]),
                color: opt_str(&l["color"]),
            })
            .collect(),
    }
}

pub(super) async fn list(
    host: &dyn Host,
    repo: &Repository,
    state: &str,
) -> Result<Vec<PullSummary>> {
    let filter = match state {
        "open" => "open",
        "all" => "all",
        _ => "closed",
    };
    let value = host
        .request(
            "GET",
            &format!(
                "{}/pulls?state={filter}&sort=recentupdate&limit=50",
                root(repo)
            ),
            None,
        )
        .await?;
    let mut pulls: Vec<PullSummary> = value
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| summary(repo, p))
        .filter(|p| match state {
            "merged" => p.state == "merged",
            "closed" => p.state == "closed",
            _ => true,
        })
        .collect();
    // The list has no CI state; read the combined status of open PRs only.
    let statuses: Vec<(usize, Option<String>)> = stream::iter(
        pulls
            .iter()
            .enumerate()
            .filter(|(_, p)| p.state == "open" && !p.head_sha.is_empty())
            .map(|(i, p)| {
                (
                    i,
                    format!("{}/commits/{}/status", root(repo), enc(&p.head_sha)),
                )
            })
            .collect::<Vec<_>>(),
    )
    .map(|(i, path)| async move {
        (
            i,
            host.request("GET", &path, None)
                .await
                .ok()
                .and_then(|v| combined(&v)),
        )
    })
    .buffer_unordered(6)
    .collect()
    .await;
    for (i, checks) in statuses {
        pulls[i].checks = checks;
    }
    Ok(pulls)
}

fn review_state(value: &Value) -> String {
    match value.as_str().unwrap_or("") {
        "APPROVED" => "approved",
        "REQUEST_CHANGES" => "changesRequested",
        "REQUEST_REVIEW" => "requested",
        "PENDING" => "pending",
        _ => "commented",
    }
    .into()
}

/// Gitea review comments carry the new-side line in `position` and the old
/// side in `original_position` when the comment is on a removed line.
fn comment_anchor(c: &Value) -> (String, u64) {
    match c["position"].as_u64().filter(|p| *p > 0) {
        Some(line) => ("right".into(), line),
        None => ("left".into(), c["original_position"].as_u64().unwrap_or(0)),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn map_detail(
    repo: &Repository,
    p: &Value,
    repo_info: &Value,
    viewer: Option<String>,
    reviews: &[Value],
    review_comments: &[Value],
    comments: &[Value],
    commits: &[Value],
    status: &Value,
) -> PullDetail {
    let mut summary = summary(repo, p);
    summary.checks = combined(status);
    let head = summary.head_sha.clone();
    let mut reviewers: Vec<Reviewer> = Vec::new();
    let mut timeline = Vec::new();
    for r in reviews {
        let state = review_state(&r["state"]);
        let login = str_of(&r["user"]["login"]);
        if r["dismissed"] != true && !login.is_empty() && state != "pending" {
            reviewers.retain(|x| x.login != login);
            reviewers.push(Reviewer {
                login: login.clone(),
                state: state.clone(),
                team: false,
            });
        }
        if matches!(state.as_str(), "pending" | "requested")
            || (state == "commented" && str_of(&r["body"]).trim().is_empty())
        {
            continue;
        }
        timeline.push(TimelineEntry {
            kind: "review".into(),
            id: r["id"].to_string(),
            author: person(
                r["user"]["login"].as_str(),
                r["user"]["avatar_url"].as_str(),
            ),
            body: str_of(&r["body"]),
            created_at: str_of(&r["submitted_at"]),
            url: opt_str(&r["html_url"]),
            state: Some(if r["dismissed"] == true {
                "dismissed".into()
            } else {
                state
            }),
            commit: opt_str(&r["commit_id"]),
        });
    }
    for login in &summary.requested_reviewers {
        reviewers.retain(|x| &x.login != login);
        reviewers.push(Reviewer {
            login: login.clone(),
            state: "requested".into(),
            team: false,
        });
    }
    let decision = if reviewers.iter().any(|r| r.state == "changesRequested") {
        Some("changesRequested")
    } else if reviewers.iter().any(|r| r.state == "approved") {
        Some("approved")
    } else if reviewers.iter().any(|r| r.state == "requested") {
        Some("reviewRequired")
    } else {
        None
    };
    summary.review_decision = decision.map(Into::into);
    for c in comments {
        timeline.push(TimelineEntry {
            kind: "comment".into(),
            id: c["id"].to_string(),
            author: person(
                c["user"]["login"].as_str(),
                c["user"]["avatar_url"].as_str(),
            ),
            body: str_of(&c["body"]),
            created_at: str_of(&c["created_at"]),
            url: opt_str(&c["html_url"]),
            state: None,
            commit: None,
        });
    }
    timeline.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let mut threads: Vec<ReviewThread> = Vec::new();
    let mut sorted = review_comments.to_vec();
    sorted.sort_by_key(|c| c["id"].as_u64().unwrap_or(0));
    for c in &sorted {
        let path = str_of(&c["path"]);
        let (side, line) = comment_anchor(c);
        let comment = ThreadComment {
            id: c["id"].to_string(),
            author: person(
                c["user"]["login"].as_str(),
                c["user"]["avatar_url"].as_str(),
            ),
            body: str_of(&c["body"]),
            created_at: str_of(&c["created_at"]),
            url: opt_str(&c["html_url"]),
        };
        if let Some(thread) = threads
            .iter_mut()
            .find(|t| t.path == path && t.side == side && t.line == Some(line))
        {
            thread.resolved = thread.resolved || c["resolver"].is_object();
            thread.comments.push(comment);
            continue;
        }
        threads.push(ReviewThread {
            id: format!("c{}", c["id"]),
            path,
            line: Some(line),
            start_line: None,
            side,
            subject: "line".into(),
            resolved: c["resolver"].is_object(),
            outdated: opt_str(&c["commit_id"]).is_some_and(|id| id != head),
            can_resolve: false,
            can_reply: true,
            comments: vec![comment],
        });
    }
    let commits = commits
        .iter()
        .map(|c| Commit {
            sha: str_of(&c["sha"]),
            message: str_of(&c["commit"]["message"]),
            author: str_of(&c["commit"]["author"]["name"]),
            login: opt_str(&c["author"]["login"]),
            date: str_of(&c["commit"]["author"]["date"]),
            url: opt_str(&c["html_url"]),
        })
        .collect();
    let check_runs = status["statuses"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| Check {
            id: format!("status:{}", s["id"]),
            name: str_of(&s["context"]),
            state: match s["status"].as_str().or(s["state"].as_str()).unwrap_or("") {
                "success" | "warning" => "passed",
                "failure" | "error" => "failed",
                _ => "pending",
            }
            .into(),
            url: opt_str(&s["target_url"]).filter(|u| u.starts_with("http")),
            description: opt_str(&s["description"]),
            started_at: opt_str(&s["created_at"]),
            completed_at: opt_str(&s["updated_at"]),
            rerun: None,
        })
        .collect();
    let mut merge_methods = Vec::new();
    for (key, method) in [
        ("allow_squash_merge", "squash"),
        ("allow_merge_commits", "merge"),
        ("allow_rebase", "rebase"),
    ] {
        if repo_info[key] != false {
            merge_methods.push(method.to_owned());
        }
    }
    let writer =
        repo_info["permissions"]["push"] == true || repo_info["permissions"]["admin"] == true;
    let mine =
        viewer.is_some() && viewer.as_deref() == summary.author.as_ref().map(|a| a.login.as_str());
    let open = summary.state == "open";
    PullDetail {
        body: str_of(&p["body"]),
        node_id: None,
        base_sha: opt_str(&p["base"]["sha"]).or(opt_str(&p["merge_base"])),
        mergeable: p["mergeable"].as_bool(),
        merge_state: Some(
            if !open {
                "unknown"
            } else if p["mergeable"] == false {
                "conflicts"
            } else if summary.checks.as_deref() == Some("failed") {
                "unstable"
            } else {
                "clean"
            }
            .into(),
        ),
        auto_merge: None,
        reviewers,
        timeline,
        threads,
        commits,
        check_runs,
        viewer,
        capabilities: Capabilities {
            // Forgejo refuses an author's own approval; leave the choice to the host.
            submit: vec!["comment".into(), "approve".into(), "requestChanges".into()],
            line_comments: true,
            file_comments: false,
            reply: true,
            resolve: false,
            viewed_native: false,
            reviewers: writer && open,
            convert_to_draft: writer || mine,
            ready: writer || mine,
            close: writer || mine,
            reopen: writer || mine,
            auto_merge: writer && open,
            merge: writer,
            revert: false,
            rerun: false,
            remove_credits: true,
        },
        merge_methods,
        // The head branch's own repository: forks fetch from there.
        extra: json!({"headClone": p["head"]["repo"]["clone_url"]}),
        summary,
    }
}

pub(super) async fn detail(host: &dyn Host, repo: &Repository, number: u64) -> Result<PullDetail> {
    let item = format!("{}/pulls/{number}", root(repo));
    let p = host.request("GET", &item, None).await?;
    ensure!(p["number"].as_u64() == Some(number), "PR not found");
    let head = str_of(&p["head"]["sha"]);
    let paths = [
        root(repo),
        format!("{item}/reviews?limit=100"),
        format!("{}/issues/{number}/comments?limit=100", root(repo)),
        format!("{item}/commits?limit=100"),
        format!("{}/commits/{}/status", root(repo), enc(&head)),
    ];
    let (repo_info, viewer, reviews, comments, commits, status) = tokio::join!(
        host.request("GET", &paths[0], None),
        host.request("GET", "user", None),
        host.request("GET", &paths[1], None),
        host.request("GET", &paths[2], None),
        host.request("GET", &paths[3], None),
        host.request("GET", &paths[4], None),
    );
    let reviews = reviews?.as_array().cloned().unwrap_or_default();
    let mut review_comments = Vec::new();
    for review in reviews
        .iter()
        .filter(|r| r["comments_count"].as_u64().unwrap_or(1) > 0)
    {
        let id = review["id"].as_u64().unwrap_or(0);
        if let Ok(list) = host
            .request("GET", &format!("{item}/reviews/{id}/comments"), None)
            .await
        {
            review_comments.extend(list.as_array().cloned().unwrap_or_default());
        }
    }
    Ok(map_detail(
        repo,
        &p,
        &repo_info.unwrap_or_default(),
        viewer.ok().and_then(|v| opt_str(&v["login"])),
        &reviews,
        &review_comments,
        &comments?.as_array().cloned().unwrap_or_default(),
        &commits?.as_array().cloned().unwrap_or_default(),
        &status.unwrap_or_default(),
    ))
}

pub(super) async fn files(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
) -> Result<Vec<PullFile>> {
    let diff = host
        .request_text(&format!("{}/pulls/{number}.diff", root(repo)))
        .await?;
    Ok(files_from_diff(&diff))
}

fn event(value: &str) -> &'static str {
    match value {
        "approve" => "APPROVED",
        "requestChanges" => "REQUEST_CHANGES",
        _ => "COMMENT",
    }
}

pub(super) fn review_body(detail: &PullDetail, input: &Submission) -> Value {
    let comments = input
        .comments
        .iter()
        .filter(|d| d.kind == "line")
        .map(|d| {
            let line = d.line.unwrap_or(0);
            // One line per comment: a range is anchored on its last line and says so.
            let body = match d.start_line.filter(|s| *s != line) {
                Some(start) => format!("Lines {start}–{line}:\n\n{}", d.body),
                None => d.body.clone(),
            };
            let mut c = json!({"path":d.path,"body":body});
            if d.side.as_deref() == Some("left") {
                c["old_position"] = json!(line);
                c["new_position"] = json!(0);
            } else {
                c["new_position"] = json!(line);
                c["old_position"] = json!(0);
            }
            c
        })
        .collect::<Vec<_>>();
    json!({
        "event": event(input.event),
        "body": fold_file_comments(&detail.capabilities, input.body, input.comments),
        "commit_id": input.head,
        "comments": comments,
    })
}

pub(super) async fn submit(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    input: &Submission<'_>,
) -> Result<Value> {
    ensure!(
        input.event != "requestChanges" || !input.body.trim().is_empty(),
        "Forgejo needs a summary to request changes"
    );
    host.request(
        "POST",
        &format!("{}/pulls/{}/reviews", root(repo), detail.summary.number),
        Some(review_body(detail, input)),
    )
    .await
}

pub(super) async fn reply(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    thread: &ReviewThread,
    body: &str,
) -> Result<Value> {
    let draft = Draft {
        id: String::new(),
        kind: "line".into(),
        path: Some(thread.path.clone()),
        side: Some(thread.side.clone()),
        line: thread.line,
        start_line: None,
        body: body.into(),
        head_sha: None,
        created_at: String::new(),
        updated_at: String::new(),
    };
    let input = Submission {
        event: "comment",
        body: "",
        comments: std::slice::from_ref(&draft),
        head: &detail.summary.head_sha,
    };
    host.request(
        "POST",
        &format!("{}/pulls/{}/reviews", root(repo), detail.summary.number),
        Some(review_body(detail, &input)),
    )
    .await
}

pub(super) async fn reviewers(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
    add: &[String],
    remove: &[String],
) -> Result<Value> {
    let path = format!("{}/pulls/{number}/requested_reviewers", root(repo));
    let split = |names: &[String]| {
        let (teams, users): (Vec<_>, Vec<_>) = names.iter().partition(|n| n.contains('/'));
        json!({"reviewers":users,"team_reviewers":teams.iter().map(|t| t.rsplit('/').next().unwrap_or("")).collect::<Vec<_>>()})
    };
    if !add.is_empty() {
        host.request("POST", &path, Some(split(add))).await?;
    }
    if !remove.is_empty() {
        host.request("DELETE", &path, Some(split(remove))).await?;
    }
    Ok(json!({"ok":true}))
}

pub(super) async fn set_state(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    state: &str,
) -> Result<Value> {
    let title = &detail.summary.title;
    let body = match state {
        "close" => json!({"state":"closed"}),
        "reopen" => json!({"state":"open"}),
        "draft" => json!({"title":format!("WIP: {title}")}),
        _ => {
            let mut t = title.as_str();
            for prefix in ["WIP:", "[WIP]", "Draft:", "wip:", "[wip]", "draft:"] {
                t = t.strip_prefix(prefix).unwrap_or(t).trim_start();
            }
            json!({"title":t})
        }
    };
    host.request(
        "PATCH",
        &format!("{}/pulls/{}", root(repo), detail.summary.number),
        Some(body),
    )
    .await
}

pub(super) async fn merge(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    input: &MergeInput<'_>,
    message: Option<(String, String)>,
) -> Result<Value> {
    // `head_commit_id` makes Forgejo refuse a head that moved after our check.
    let mut body = json!({"Do":input.method,"head_commit_id":input.expected_head});
    if let Some((title, message)) = message {
        body["MergeTitleField"] = json!(title);
        body["MergeMessageField"] = json!(message);
    }
    host.request(
        "POST",
        &format!("{}/pulls/{}/merge", root(repo), detail.summary.number),
        Some(body),
    )
    .await
    .map_err(merge_error)
}

/// Forgejo answers 409 when the head moved and 405 while it is still checking
/// mergeability after a push (or when branch rules block the merge).
fn merge_error(error: anyhow::Error) -> anyhow::Error {
    let text = error.to_string();
    if text.contains("HTTP 409") {
        anyhow::anyhow!("conflict: PR head moved; load new commits before merging")
    } else if text.contains("HTTP 405") {
        anyhow::anyhow!("Forgejo cannot merge this PR yet: it may still be checking the latest push, or a branch rule blocks it. Try again in a moment")
    } else {
        error
    }
}

pub(super) async fn auto_merge(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    input: &MergeInput<'_>,
    enable: bool,
) -> Result<Value> {
    let path = format!("{}/pulls/{}/merge", root(repo), detail.summary.number);
    if !enable {
        return host.request("DELETE", &path, None).await;
    }
    host.request(
        "POST",
        &path,
        Some(json!({"Do":input.method,"head_commit_id":input.expected_head,"merge_when_checks_succeed":true})),
    )
    .await
}
