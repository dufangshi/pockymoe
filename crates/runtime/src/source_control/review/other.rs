//! Bitbucket Cloud and Azure DevOps: reading, commenting and review votes,
//! where each is one cheap call. Bitbucket merges stay refused (no atomic head
//! guard); Azure files come without patches (its API serves no unified diff).
use super::*;
use crate::source_control::hosts::Host;

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

// ---------------------------------------------------------------- Bitbucket

fn bb_root(repo: &Repository) -> String {
    format!(
        "repositories/{}/{}/pullrequests",
        enc(&repo.owner),
        enc(&repo.name)
    )
}
fn bb_state(value: &Value) -> String {
    match value.as_str().unwrap_or("") {
        "MERGED" => "merged",
        "DECLINED" | "SUPERSEDED" => "closed",
        _ => "open",
    }
    .into()
}
fn bb_person(user: &Value) -> Option<Person> {
    person(
        user["nickname"].as_str().or(user["display_name"].as_str()),
        user["links"]["avatar"]["href"].as_str(),
    )
}

fn bb_summary(repo: &Repository, p: &Value) -> PullSummary {
    let participants = p["participants"].as_array().cloned().unwrap_or_default();
    let decision = if participants
        .iter()
        .any(|x| x["state"] == "changes_requested")
    {
        Some("changesRequested".into())
    } else if participants.iter().any(|x| x["approved"] == true) {
        Some("approved".into())
    } else {
        None
    };
    PullSummary {
        url: str_of(&p["links"]["html"]["href"]),
        number: p["id"].as_u64().unwrap_or(0),
        title: str_of(&p["title"]),
        state: bb_state(&p["state"]),
        draft: p["draft"] == true,
        author: bb_person(&p["author"]),
        repository: repo.clone(),
        head_ref: str_of(&p["source"]["branch"]["name"]),
        base_ref: str_of(&p["destination"]["branch"]["name"]),
        head_sha: str_of(&p["source"]["commit"]["hash"]),
        created_at: str_of(&p["created_on"]),
        updated_at: str_of(&p["updated_on"]),
        checks: None,
        review_decision: decision,
        requested_reviewers: p["reviewers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| bb_person(r).map(|p| p.login))
            .collect(),
        additions: None,
        deletions: None,
        changed_files: None,
        comments: p["comment_count"].as_u64(),
        labels: Vec::new(),
    }
}

pub(super) async fn bitbucket_list(
    host: &dyn Host,
    repo: &Repository,
    state: &str,
) -> Result<Vec<PullSummary>> {
    let filter = match state {
        "open" => "state=OPEN",
        "merged" => "state=MERGED",
        "closed" => "state=DECLINED",
        _ => "state=OPEN&state=MERGED&state=DECLINED",
    };
    let value = host
        .request(
            "GET",
            &format!("{}?{filter}&pagelen=50", bb_root(repo)),
            None,
        )
        .await?;
    Ok(value["values"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| bb_summary(repo, p))
        .collect())
}

pub(super) async fn bitbucket_detail(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
) -> Result<PullDetail> {
    let item = format!("{}/{number}", bb_root(repo));
    let p = host.request("GET", &item, None).await?;
    ensure!(p["id"].as_u64() == Some(number), "PR not found");
    let paths = [
        format!("{item}/comments?pagelen=100"),
        format!("{item}/commits?pagelen=100"),
        format!("{item}/statuses?pagelen=100"),
    ];
    let (comments, commits, statuses, viewer) = tokio::join!(
        host.request("GET", &paths[0], None),
        host.request("GET", &paths[1], None),
        host.request("GET", &paths[2], None),
        host.request("GET", "user", None),
    );
    Ok(bitbucket_map(
        repo,
        &p,
        &comments?["values"],
        &commits.unwrap_or_default()["values"],
        &statuses.unwrap_or_default()["values"],
        viewer.ok().and_then(|v| opt_str(&v["nickname"])),
    ))
}

pub(super) fn bitbucket_map(
    repo: &Repository,
    p: &Value,
    comments: &Value,
    commits: &Value,
    statuses: &Value,
    viewer: Option<String>,
) -> PullDetail {
    let mut summary = bb_summary(repo, p);
    let check_runs: Vec<Check> = statuses
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| Check {
            id: format!("status:{}", str_of(&s["key"])),
            name: opt_str(&s["name"]).unwrap_or_else(|| str_of(&s["key"])),
            state: match s["state"].as_str().unwrap_or("") {
                "SUCCESSFUL" => "passed",
                "FAILED" => "failed",
                "STOPPED" => "cancelled",
                _ => "pending",
            }
            .into(),
            url: opt_str(&s["url"]).filter(|u| u.starts_with("https://")),
            description: opt_str(&s["description"]),
            started_at: opt_str(&s["created_on"]),
            completed_at: opt_str(&s["updated_on"]),
            rerun: None,
        })
        .collect();
    summary.checks = Some(if check_runs.iter().any(|c| c.state == "failed") {
        "failed".into()
    } else if check_runs.iter().any(|c| c.state == "pending") {
        "pending".into()
    } else if check_runs.is_empty() {
        "none".into()
    } else {
        "passed".into()
    });
    let mut reviewers = Vec::new();
    for x in p["participants"].as_array().into_iter().flatten() {
        if let Some(user) = bb_person(&x["user"]) {
            let state = if x["state"] == "changes_requested" {
                "changesRequested"
            } else if x["approved"] == true {
                "approved"
            } else if x["role"] == "REVIEWER" {
                "requested"
            } else {
                continue;
            };
            reviewers.push(Reviewer {
                login: user.login,
                state: state.into(),
                team: false,
            });
        }
    }
    let mut timeline = Vec::new();
    let mut threads: Vec<ReviewThread> = Vec::new();
    let list = comments.as_array().cloned().unwrap_or_default();
    for c in list.iter().filter(|c| c["deleted"] != true) {
        let comment = ThreadComment {
            id: c["id"].to_string(),
            author: bb_person(&c["user"]),
            body: str_of(&c["content"]["raw"]),
            created_at: str_of(&c["created_on"]),
            url: opt_str(&c["links"]["html"]["href"]),
        };
        if let Some(parent) = c["parent"]["id"].as_u64() {
            if let Some(thread) = threads
                .iter_mut()
                .find(|t| t.comments.iter().any(|x| x.id == parent.to_string()))
            {
                thread.comments.push(comment);
                continue;
            }
        }
        if c["inline"].is_object() {
            let inline = &c["inline"];
            let right = inline["to"].as_u64().is_some();
            threads.push(ReviewThread {
                id: c["id"].to_string(),
                path: str_of(&inline["path"]),
                line: inline["to"].as_u64().or(inline["from"].as_u64()),
                start_line: None,
                side: if right { "right" } else { "left" }.into(),
                subject: if inline["to"].is_null() && inline["from"].is_null() {
                    "file"
                } else {
                    "line"
                }
                .into(),
                resolved: c["resolution"].is_object(),
                outdated: false,
                can_resolve: true,
                can_reply: true,
                comments: vec![comment],
            });
        } else {
            timeline.push(TimelineEntry {
                kind: "comment".into(),
                id: comment.id,
                author: comment.author,
                body: comment.body,
                created_at: comment.created_at,
                url: comment.url,
                state: None,
                commit: None,
            });
        }
    }
    let commits = commits
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| Commit {
            sha: str_of(&c["hash"]),
            message: str_of(&c["message"]),
            author: str_of(&c["author"]["raw"])
                .split(" <")
                .next()
                .unwrap_or("")
                .to_owned(),
            login: opt_str(&c["author"]["user"]["nickname"]),
            date: str_of(&c["date"]),
            url: opt_str(&c["links"]["html"]["href"]),
        })
        .collect();
    let open = summary.state == "open";
    PullDetail {
        body: str_of(&p["description"]),
        node_id: None,
        base_sha: opt_str(&p["destination"]["commit"]["hash"]),
        mergeable: None,
        merge_state: Some("unknown".into()),
        auto_merge: None,
        reviewers,
        timeline,
        threads,
        commits,
        check_runs,
        viewer,
        capabilities: Capabilities {
            submit: vec!["comment".into(), "approve".into(), "requestChanges".into()],
            line_comments: true,
            file_comments: true,
            reply: true,
            resolve: true,
            convert_to_draft: open,
            ready: open,
            close: open,
            ..Default::default()
        },
        merge_methods: Vec::new(),
        extra: json!({"headClone": p["source"]["repository"]["links"]["clone"].as_array().and_then(|a| a.iter().find(|v| v["name"] == "https")).map(|v| v["href"].clone()).unwrap_or(Value::Null)}),
        summary,
    }
}

pub(super) async fn bitbucket_files(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
) -> Result<Vec<PullFile>> {
    let p = host
        .request("GET", &format!("{}/{number}", bb_root(repo)), None)
        .await?;
    let (src, dst) = (
        str_of(&p["source"]["commit"]["hash"]),
        str_of(&p["destination"]["commit"]["hash"]),
    );
    ensure!(
        [&src, &dst]
            .iter()
            .all(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit())),
        "PR commits unavailable"
    );
    // The PR's own /diff answers with a redirect; ask for the same spec directly.
    let diff = host
        .request_text(&format!(
            "repositories/{}/{}/diff/{src}..{dst}",
            enc(&repo.owner),
            enc(&repo.name)
        ))
        .await?;
    Ok(files_from_diff(&diff))
}

pub(super) async fn bitbucket_submit(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    input: &Submission<'_>,
) -> Result<Value> {
    let item = format!("{}/{}", bb_root(repo), detail.summary.number);
    for d in input.comments {
        let mut inline = json!({"path":d.path});
        if d.kind == "line" {
            inline[if d.side.as_deref() == Some("left") {
                "from"
            } else {
                "to"
            }] = json!(d.line);
        }
        host.request(
            "POST",
            &format!("{item}/comments"),
            Some(json!({"content":{"raw":d.body},"inline":inline})),
        )
        .await?;
    }
    if !input.body.trim().is_empty() {
        host.request(
            "POST",
            &format!("{item}/comments"),
            Some(json!({"content":{"raw":input.body}})),
        )
        .await?;
    }
    match input.event {
        "approve" => host.request("POST", &format!("{item}/approve"), None).await,
        "requestChanges" => {
            host.request("POST", &format!("{item}/request-changes"), None)
                .await
        }
        _ => Ok(json!({"ok":true})),
    }
}

pub(super) async fn bitbucket_reply(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
    thread: &ReviewThread,
    body: &str,
) -> Result<Value> {
    let parent: u64 = thread.id.parse().context("Invalid thread")?;
    host.request(
        "POST",
        &format!("{}/{number}/comments", bb_root(repo)),
        Some(json!({"content":{"raw":body},"parent":{"id":parent}})),
    )
    .await
}

pub(super) async fn bitbucket_resolve(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
    thread: &ReviewThread,
    resolved: bool,
) -> Result<Value> {
    let id: u64 = thread.id.parse().context("Invalid thread")?;
    host.request(
        if resolved { "POST" } else { "DELETE" },
        &format!("{}/{number}/comments/{id}/resolve", bb_root(repo)),
        None,
    )
    .await
}

pub(super) async fn bitbucket_state(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    state: &str,
) -> Result<Value> {
    let item = format!("{}/{}", bb_root(repo), detail.summary.number);
    match state {
        "close" => host.request("POST", &format!("{item}/decline"), None).await,
        "draft" | "ready" => {
            host.request("PUT", &item, Some(json!({"draft":state=="draft"})))
                .await
        }
        _ => bail!("Bitbucket cannot reopen a declined PR"),
    }
}

// ---------------------------------------------------------------- Azure DevOps

const API: &str = "api-version=7.1";
fn az_root(repo: &Repository) -> String {
    format!(
        "{}/_apis/git/repositories/{}/pullrequests",
        repo.owner.trim_end_matches("/_git"),
        enc(&repo.name)
    )
}
fn az_person(user: &Value) -> Option<Person> {
    person(
        user["uniqueName"].as_str().or(user["displayName"].as_str()),
        user["imageUrl"].as_str(),
    )
}
fn az_state(value: &Value) -> String {
    match value.as_str().unwrap_or("") {
        "completed" => "merged",
        "abandoned" => "closed",
        _ => "open",
    }
    .into()
}
fn vote_state(vote: i64) -> &'static str {
    match vote {
        v if v >= 5 => "approved",
        -10 | -5 => "changesRequested",
        _ => "requested",
    }
}

fn az_summary(repo: &Repository, p: &Value) -> PullSummary {
    let number = p["pullRequestId"].as_u64().unwrap_or(0);
    let votes: Vec<i64> = p["reviewers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["vote"].as_i64())
        .collect();
    PullSummary {
        url: repo.pull_request_url(number),
        number,
        title: str_of(&p["title"]),
        state: az_state(&p["status"]),
        draft: p["isDraft"] == true,
        author: az_person(&p["createdBy"]),
        repository: repo.clone(),
        head_ref: str_of(&p["sourceRefName"])
            .trim_start_matches("refs/heads/")
            .into(),
        base_ref: str_of(&p["targetRefName"])
            .trim_start_matches("refs/heads/")
            .into(),
        head_sha: str_of(&p["lastMergeSourceCommit"]["commitId"]),
        created_at: str_of(&p["creationDate"]),
        updated_at: opt_str(&p["closedDate"]).unwrap_or_else(|| str_of(&p["creationDate"])),
        checks: None,
        review_decision: if votes.iter().any(|v| *v < 0) {
            Some("changesRequested".into())
        } else if votes.iter().any(|v| *v >= 5) {
            Some("approved".into())
        } else {
            None
        },
        requested_reviewers: p["reviewers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| az_person(r).map(|p| p.login))
            .collect(),
        additions: None,
        deletions: None,
        changed_files: None,
        comments: None,
        labels: p["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|l| Label {
                name: str_of(&l["name"]),
                color: None,
            })
            .collect(),
    }
}

pub(super) async fn azure_list(
    host: &dyn Host,
    repo: &Repository,
    state: &str,
) -> Result<Vec<PullSummary>> {
    let status = match state {
        "open" => "active",
        "merged" => "completed",
        "closed" => "abandoned",
        _ => "all",
    };
    let value = host
        .request(
            "GET",
            &format!(
                "{}?searchCriteria.status={status}&$top=50&{API}",
                az_root(repo)
            ),
            None,
        )
        .await?;
    Ok(value["value"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| az_summary(repo, p))
        .collect())
}

pub(super) async fn azure_detail(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
) -> Result<PullDetail> {
    let item = format!("{}/{number}", az_root(repo));
    let p = host.request("GET", &format!("{item}?{API}"), None).await?;
    ensure!(p["pullRequestId"].as_u64() == Some(number), "PR not found");
    let identity = format!(
        "{}/_apis/connectionData",
        repo.owner.split('/').next().unwrap_or("")
    );
    let paths = [
        format!("{item}/threads?{API}"),
        format!("{item}/commits?{API}"),
        format!("{item}/statuses?{API}"),
    ];
    let (threads, commits, statuses, viewer) = tokio::join!(
        host.request("GET", &paths[0], None),
        host.request("GET", &paths[1], None),
        host.request("GET", &paths[2], None),
        host.request("GET", &identity, None),
    );
    let viewer = viewer.unwrap_or_default();
    Ok(azure_map(
        repo,
        &p,
        &threads?["value"],
        &commits.unwrap_or_default()["value"],
        &statuses.unwrap_or_default()["value"],
        &viewer["authenticatedUser"],
    ))
}

pub(super) fn azure_map(
    repo: &Repository,
    p: &Value,
    threads_value: &Value,
    commits: &Value,
    statuses: &Value,
    viewer: &Value,
) -> PullDetail {
    let mut summary = az_summary(repo, p);
    let check_runs: Vec<Check> = statuses
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| Check {
            id: format!("status:{}", s["id"]),
            name: format!(
                "{}/{}",
                str_of(&s["context"]["genre"]),
                str_of(&s["context"]["name"])
            )
            .trim_start_matches('/')
            .into(),
            state: match s["state"].as_str().unwrap_or("") {
                "succeeded" => "passed",
                "failed" | "error" => "failed",
                "notApplicable" => "skipped",
                _ => "pending",
            }
            .into(),
            url: opt_str(&s["targetUrl"]).filter(|u| u.starts_with("https://")),
            description: opt_str(&s["description"]),
            started_at: opt_str(&s["creationDate"]),
            completed_at: None,
            rerun: None,
        })
        .collect();
    summary.checks = Some(if check_runs.iter().any(|c| c.state == "failed") {
        "failed".into()
    } else if check_runs.iter().any(|c| c.state == "pending") {
        "pending".into()
    } else if check_runs.is_empty() {
        "none".into()
    } else {
        "passed".into()
    });
    let reviewers = p["reviewers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some(Reviewer {
                login: az_person(r)?.login,
                state: vote_state(r["vote"].as_i64().unwrap_or(0)).into(),
                team: r["isContainer"] == true,
            })
        })
        .collect();
    let mut timeline = Vec::new();
    let mut threads = Vec::new();
    for t in threads_value.as_array().into_iter().flatten() {
        if t["isDeleted"] == true {
            continue;
        }
        let comments: Vec<ThreadComment> = t["comments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["commentType"] != "system" && c["isDeleted"] != true)
            .map(|c| ThreadComment {
                id: c["id"].to_string(),
                author: az_person(&c["author"]),
                body: str_of(&c["content"]),
                created_at: str_of(&c["publishedDate"]),
                url: None,
            })
            .collect();
        if comments.is_empty() {
            continue;
        }
        let context = &t["threadContext"];
        if let Some(path) = opt_str(&context["filePath"]) {
            let right = context["rightFileStart"].is_object();
            let start = if right {
                &context["rightFileStart"]
            } else {
                &context["leftFileStart"]
            };
            let end = if right {
                &context["rightFileEnd"]
            } else {
                &context["leftFileEnd"]
            };
            threads.push(ReviewThread {
                id: t["id"].to_string(),
                path: path.trim_start_matches('/').into(),
                line: end["line"].as_u64().or(start["line"].as_u64()),
                start_line: start["line"]
                    .as_u64()
                    .filter(|s| Some(*s) != end["line"].as_u64()),
                side: if right { "right" } else { "left" }.into(),
                subject: if start.is_object() { "line" } else { "file" }.into(),
                resolved: !matches!(t["status"].as_str(), Some("active" | "pending") | None),
                outdated: false,
                can_resolve: true,
                can_reply: true,
                comments,
            });
        } else {
            for c in comments {
                timeline.push(TimelineEntry {
                    kind: "comment".into(),
                    id: format!("{}:{}", t["id"], c.id),
                    author: c.author,
                    body: c.body,
                    created_at: c.created_at,
                    url: None,
                    state: None,
                    commit: None,
                });
            }
        }
    }
    timeline.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let commits = commits
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| Commit {
            sha: str_of(&c["commitId"]),
            message: str_of(&c["comment"]),
            author: str_of(&c["author"]["name"]),
            login: None,
            date: str_of(&c["author"]["date"]),
            url: opt_str(&c["remoteUrl"]),
        })
        .collect();
    let open = summary.state == "open";
    PullDetail {
        body: str_of(&p["description"]),
        node_id: None,
        base_sha: opt_str(&p["lastMergeTargetCommit"]["commitId"]),
        mergeable: match p["mergeStatus"].as_str() {
            Some("succeeded") => Some(true),
            Some("conflicts") => Some(false),
            _ => None,
        },
        merge_state: Some(
            match p["mergeStatus"].as_str() {
                Some("succeeded") => "clean",
                Some("conflicts") => "conflicts",
                _ => "unknown",
            }
            .into(),
        ),
        auto_merge: p["autoCompleteSetBy"].is_object().then(|| "merge".into()),
        reviewers,
        timeline,
        threads,
        commits,
        check_runs,
        viewer: opt_str(&viewer["providerDisplayName"]),
        capabilities: Capabilities {
            submit: vec!["comment".into(), "approve".into(), "requestChanges".into()],
            line_comments: true,
            file_comments: true,
            reply: true,
            resolve: true,
            convert_to_draft: open,
            ready: open,
            close: open,
            reopen: summary.state == "closed",
            merge: open,
            remove_credits: true,
            ..Default::default()
        },
        merge_methods: vec!["squash".into(), "merge".into(), "rebase".into()],
        extra: json!({"viewerId": viewer["id"], "headClone": p["forkSource"]["repository"]["remoteUrl"]}),
        summary,
    }
}

pub(super) async fn azure_files(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
) -> Result<Vec<PullFile>> {
    let item = format!("{}/{number}", az_root(repo));
    let iterations = host
        .request("GET", &format!("{item}/iterations?{API}"), None)
        .await?;
    let last = iterations["value"]
        .as_array()
        .and_then(|a| a.last())
        .and_then(|i| i["id"].as_u64())
        .context("PR iterations unavailable")?;
    let changes = host
        .request(
            "GET",
            &format!("{item}/iterations/{last}/changes?{API}"),
            None,
        )
        .await?;
    Ok(changes["changeEntries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["item"]["isFolder"] != true)
        .map(|c| {
            let path = str_of(&c["item"]["path"])
                .trim_start_matches('/')
                .to_owned();
            let kind = str_of(&c["changeType"]);
            PullFile {
                digest: opt_str(&c["item"]["objectId"]).unwrap_or_else(|| digest(&path)),
                old_path: opt_str(&c["originalPath"]).map(|p| p.trim_start_matches('/').to_owned()),
                status: file_status(kind.split(',').next().unwrap_or("").trim()),
                additions: 0,
                deletions: 0,
                patch: None,
                binary: false,
                viewed: "unviewed".into(),
                path,
            }
        })
        .collect())
}

fn az_context(d: &Draft) -> Value {
    let path = format!(
        "/{}",
        d.path.as_deref().unwrap_or("").trim_start_matches('/')
    );
    if d.kind != "line" {
        return json!({"filePath":path});
    }
    let line = d.line.unwrap_or(1);
    let start = d.start_line.unwrap_or(line);
    let (from, to) = if d.side.as_deref() == Some("left") {
        ("leftFileStart", "leftFileEnd")
    } else {
        ("rightFileStart", "rightFileEnd")
    };
    json!({"filePath":path, from:{"line":start,"offset":1}, to:{"line":line,"offset":1}})
}

pub(super) async fn azure_submit(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    input: &Submission<'_>,
) -> Result<Value> {
    let item = format!("{}/{}", az_root(repo), detail.summary.number);
    let viewer = detail.extra["viewerId"]
        .as_str()
        .filter(|_| input.event != "comment")
        .map(str::to_owned);
    ensure!(
        input.event == "comment" || viewer.is_some(),
        "Azure identity unavailable"
    );
    for d in input.comments {
        host.request(
            "POST",
            &format!("{item}/threads?{API}"),
            Some(json!({"comments":[{"parentCommentId":0,"content":d.body,"commentType":1}],"status":1,"threadContext":az_context(d)})),
        )
        .await?;
    }
    if !input.body.trim().is_empty() {
        host.request(
            "POST",
            &format!("{item}/threads?{API}"),
            Some(json!({"comments":[{"parentCommentId":0,"content":input.body,"commentType":1}],"status":1})),
        )
        .await?;
    }
    match viewer {
        Some(id) => {
            host.request(
                "PUT",
                &format!("{item}/reviewers/{}?{API}", enc(&id)),
                Some(json!({"vote":if input.event=="approve"{10}else{-5}})),
            )
            .await
        }
        None => Ok(json!({"ok":true})),
    }
}

pub(super) async fn azure_reply(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
    thread: &ReviewThread,
    body: &str,
) -> Result<Value> {
    let id: u64 = thread.id.parse().context("Invalid thread")?;
    host.request(
        "POST",
        &format!("{}/{number}/threads/{id}/comments?{API}", az_root(repo)),
        Some(json!({"content":body,"parentCommentId":1,"commentType":1})),
    )
    .await
}

pub(super) async fn azure_resolve(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
    thread: &str,
    resolved: bool,
) -> Result<Value> {
    let id: u64 = thread.parse().context("Invalid thread")?;
    host.request(
        "PATCH",
        &format!("{}/{number}/threads/{id}?{API}", az_root(repo)),
        Some(json!({"status":if resolved{"fixed"}else{"active"}})),
    )
    .await
}

pub(super) async fn azure_state(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    state: &str,
) -> Result<Value> {
    let body = match state {
        "close" => json!({"status":"abandoned"}),
        "reopen" => json!({"status":"active"}),
        "draft" => json!({"isDraft":true}),
        _ => json!({"isDraft":false}),
    };
    host.request(
        "PATCH",
        &format!("{}/{}?{API}", az_root(repo), detail.summary.number),
        Some(body),
    )
    .await
}

pub(super) async fn azure_merge(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    input: &MergeInput<'_>,
    message: Option<(String, String)>,
) -> Result<Value> {
    let strategy = match input.method {
        "rebase" => "rebase",
        "squash" => "squash",
        _ => "noFastForward",
    };
    let mut options = json!({"mergeStrategy":strategy});
    if let Some((title, body)) = message {
        options["mergeCommitMessage"] = json!(format!("{title}\n\n{body}").trim_end());
    }
    // `lastMergeSourceCommit` makes Azure refuse a head that moved.
    host.request(
        "PATCH",
        &format!("{}/{}?{API}", az_root(repo), detail.summary.number),
        Some(json!({"status":"completed","lastMergeSourceCommit":{"commitId":input.expected_head},"completionOptions":options})),
    )
    .await
}
