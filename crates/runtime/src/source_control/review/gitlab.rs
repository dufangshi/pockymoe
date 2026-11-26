//! GitLab merge requests: GraphQL for the list (pipeline state per MR), the
//! default merge messages and "request changes"; REST for the rest. Inline
//! comments are discussions with a diff position built from the MR's diff refs.
use super::*;
use crate::source_control::hosts::Host;

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
fn project(repo: &Repository) -> String {
    format!("projects/{}", enc(&format!("{}/{}", repo.owner, repo.name)))
}
fn item(repo: &Repository, n: u64) -> String {
    format!("{}/merge_requests/{n}", project(repo))
}
fn full_path(repo: &Repository) -> String {
    format!("{}/{}", repo.owner, repo.name)
}
fn state(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "merged" => "merged",
        "closed" | "locked" => "closed",
        _ => "open",
    }
    .into()
}
fn pipeline(value: &Value) -> Option<String> {
    Some(
        match value.as_str() {
            None => "none",
            Some(s) => match s.to_ascii_lowercase().as_str() {
                "success" => "passed",
                "failed" => "failed",
                "canceled" | "skipped" | "manual" => "none",
                _ => "pending",
            },
        }
        .into(),
    )
}

pub(super) async fn list(
    host: &dyn Host,
    repo: &Repository,
    state_filter: &str,
) -> Result<Vec<PullSummary>> {
    let filter = match state_filter {
        "open" => "opened",
        other => other,
    };
    let data = graphql(
        host,
        "query($fullPath:ID!,$state:MergeRequestState,$first:Int!){project(fullPath:$fullPath){mergeRequests(state:$state,first:$first,sort:UPDATED_DESC){nodes{iid title webUrl state draft createdAt updatedAt sourceBranch targetBranch diffHeadSha author{username avatarUrl} labels(first:10){nodes{title color}} reviewers(first:20){nodes{username}} headPipeline{status} approved userNotesCount diffStatsSummary{additions deletions fileCount}}}}}",
        json!({"fullPath":full_path(repo),"state":filter,"first":50}),
    )
    .await?;
    Ok(data["project"]["mergeRequests"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|n| PullSummary {
            url: str_of(&n["webUrl"]),
            number: n["iid"]
                .as_str()
                .and_then(|i| i.parse().ok())
                .or(n["iid"].as_u64())
                .unwrap_or(0),
            title: str_of(&n["title"]),
            state: state(n["state"].as_str().unwrap_or("")),
            draft: n["draft"] == true,
            author: person(
                n["author"]["username"].as_str(),
                n["author"]["avatarUrl"].as_str(),
            ),
            repository: repo.clone(),
            head_ref: str_of(&n["sourceBranch"]),
            base_ref: str_of(&n["targetBranch"]),
            head_sha: str_of(&n["diffHeadSha"]),
            created_at: str_of(&n["createdAt"]),
            updated_at: str_of(&n["updatedAt"]),
            checks: pipeline(&n["headPipeline"]["status"]),
            review_decision: (n["approved"] == true).then(|| "approved".into()),
            requested_reviewers: n["reviewers"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| opt_str(&r["username"]))
                .collect(),
            additions: n["diffStatsSummary"]["additions"].as_u64(),
            deletions: n["diffStatsSummary"]["deletions"].as_u64(),
            changed_files: n["diffStatsSummary"]["fileCount"].as_u64(),
            comments: n["userNotesCount"].as_u64(),
            labels: n["labels"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|l| Label {
                    name: str_of(&l["title"]),
                    color: opt_str(&l["color"]).map(|c| c.trim_start_matches('#').to_owned()),
                })
                .collect(),
        })
        .collect())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn map_detail(
    repo: &Repository,
    m: &Value,
    project_info: &Value,
    viewer: Option<String>,
    discussions: &[Value],
    approvals: &Value,
    commits: &[Value],
    jobs: &[Value],
) -> PullDetail {
    let summary = PullSummary {
        url: str_of(&m["web_url"]),
        number: m["iid"].as_u64().unwrap_or(0),
        title: str_of(&m["title"]),
        state: state(m["state"].as_str().unwrap_or("")),
        draft: m["draft"] == true || m["work_in_progress"] == true,
        author: person(
            m["author"]["username"].as_str(),
            m["author"]["avatar_url"].as_str(),
        ),
        repository: repo.clone(),
        head_ref: str_of(&m["source_branch"]),
        base_ref: str_of(&m["target_branch"]),
        head_sha: str_of(&m["sha"]),
        created_at: str_of(&m["created_at"]),
        updated_at: str_of(&m["updated_at"]),
        checks: if m["head_pipeline"].is_object() {
            pipeline(&m["head_pipeline"]["status"])
        } else {
            Some("none".into())
        },
        review_decision: (approvals["approved"] == true).then(|| "approved".into()),
        requested_reviewers: m["reviewers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| opt_str(&r["username"]))
            .collect(),
        additions: None,
        deletions: None,
        changed_files: m["changes_count"]
            .as_str()
            .and_then(|c| c.trim_end_matches('+').parse().ok()),
        comments: m["user_notes_count"].as_u64(),
        labels: m["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| {
                l.as_str().map(|name| Label {
                    name: name.into(),
                    color: None,
                })
            })
            .collect(),
    };
    let head = summary.head_sha.clone();
    let approved: Vec<String> = approvals["approved_by"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| opt_str(&a["user"]["username"]))
        .collect();
    let mut reviewers: Vec<Reviewer> = summary
        .requested_reviewers
        .iter()
        .map(|login| Reviewer {
            login: login.clone(),
            state: if approved.contains(login) {
                "approved"
            } else {
                "requested"
            }
            .into(),
            team: false,
        })
        .collect();
    for login in &approved {
        if !reviewers.iter().any(|r| &r.login == login) {
            reviewers.push(Reviewer {
                login: login.clone(),
                state: "approved".into(),
                team: false,
            });
        }
    }
    let mut timeline = Vec::new();
    let mut threads = Vec::new();
    for d in discussions {
        let notes: Vec<&Value> = d["notes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|n| n["system"] != true)
            .collect();
        let Some(first) = notes.first() else { continue };
        let note_url = |n: &Value| Some(format!("{}#note_{}", summary.url, n["id"]));
        if first["type"] == "DiffNote" && first["position"].is_object() {
            let pos = &first["position"];
            let right = pos["new_line"].as_u64().is_some();
            let line = pos["new_line"].as_u64().or(pos["old_line"].as_u64());
            let start = &pos["line_range"]["start"];
            threads.push(ReviewThread {
                id: str_of(&d["id"]),
                path: opt_str(&pos["new_path"]).unwrap_or_else(|| str_of(&pos["old_path"])),
                line,
                start_line: if right {
                    start["new_line"].as_u64()
                } else {
                    start["old_line"].as_u64()
                }
                .filter(|s| Some(*s) != line),
                side: if right { "right" } else { "left" }.into(),
                subject: if pos["position_type"] == "file" {
                    "file"
                } else {
                    "line"
                }
                .into(),
                resolved: first["resolved"] == true,
                outdated: opt_str(&pos["head_sha"]).is_some_and(|h| h != head),
                can_resolve: first["resolvable"] == true,
                can_reply: true,
                comments: notes
                    .iter()
                    .map(|n| ThreadComment {
                        id: n["id"].to_string(),
                        author: person(
                            n["author"]["username"].as_str(),
                            n["author"]["avatar_url"].as_str(),
                        ),
                        body: str_of(&n["body"]),
                        created_at: str_of(&n["created_at"]),
                        url: note_url(n),
                    })
                    .collect(),
            });
            continue;
        }
        for n in notes {
            timeline.push(TimelineEntry {
                kind: "comment".into(),
                id: n["id"].to_string(),
                author: person(
                    n["author"]["username"].as_str(),
                    n["author"]["avatar_url"].as_str(),
                ),
                body: str_of(&n["body"]),
                created_at: str_of(&n["created_at"]),
                url: note_url(n),
                state: None,
                commit: None,
            });
        }
    }
    timeline.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let commits = commits
        .iter()
        .map(|c| Commit {
            sha: str_of(&c["id"]),
            message: opt_str(&c["message"]).unwrap_or_else(|| str_of(&c["title"])),
            author: str_of(&c["author_name"]),
            login: None,
            date: str_of(&c["created_at"]),
            url: opt_str(&c["web_url"]),
        })
        .collect();
    let pipeline_id = m["head_pipeline"]["id"].as_u64();
    let check_runs = jobs
        .iter()
        .map(|j| {
            let status = j["status"].as_str().unwrap_or("");
            let state = match status {
                "success" => "passed",
                "failed" => "failed",
                "canceled" => "cancelled",
                "skipped" => "skipped",
                "manual" => "neutral",
                _ => "pending",
            };
            Check {
                id: format!("job:{}", j["id"]),
                name: format!("{} · {}", str_of(&j["stage"]), str_of(&j["name"]))
                    .trim_start_matches(" · ")
                    .into(),
                state: state.into(),
                url: opt_str(&j["web_url"]).filter(|u| u.starts_with("http")),
                description: None,
                started_at: opt_str(&j["started_at"]),
                completed_at: opt_str(&j["finished_at"]),
                rerun: pipeline_id
                    .filter(|_| matches!(state, "failed" | "cancelled"))
                    .map(|id| format!("pipeline:{id}")),
            }
        })
        .collect();
    let access = project_info["permissions"]["project_access"]["access_level"]
        .as_u64()
        .max(project_info["permissions"]["group_access"]["access_level"].as_u64())
        .unwrap_or(0);
    let writer = access >= 30 || m["user"]["can_merge"] == true;
    let mine =
        viewer.is_some() && viewer.as_deref() == summary.author.as_ref().map(|a| a.login.as_str());
    let open = summary.state == "open";
    let mut merge_methods = vec!["merge".to_owned()];
    if project_info["squash_option"] != "never" {
        merge_methods.insert(0, "squash".into());
    }
    PullDetail {
        body: str_of(&m["description"]),
        node_id: None,
        base_sha: opt_str(&m["diff_refs"]["base_sha"]),
        mergeable: match m["detailed_merge_status"].as_str() {
            Some("mergeable") => Some(true),
            Some("broken_status" | "conflict") => Some(false),
            _ => (m["has_conflicts"] == true).then_some(false),
        },
        merge_state: Some(
            match m["detailed_merge_status"].as_str().unwrap_or("") {
                "mergeable" => "clean",
                "conflict" | "broken_status" => "conflicts",
                "need_rebase" => "behind",
                "ci_must_pass" | "ci_still_running" => "unstable",
                "draft_status" => "draft",
                "not_approved" | "discussions_not_resolved" | "blocked_status" => "blocked",
                _ => "unknown",
            }
            .into(),
        ),
        auto_merge: (m["merge_when_pipeline_succeeds"] == true).then(|| "merge".into()),
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
            viewed_native: false,
            reviewers: writer && open,
            convert_to_draft: writer || mine,
            ready: writer || mine,
            close: writer || mine,
            reopen: writer || mine,
            auto_merge: writer && open,
            merge: writer,
            revert: false,
            rerun: writer,
            remove_credits: true,
        },
        merge_methods,
        extra: json!({
            "diffRefs": m["diff_refs"],
            "reviewerIds": m["reviewers"].as_array().into_iter().flatten().map(|r| json!({"login":r["username"],"id":r["id"]})).collect::<Vec<_>>(),
        }),
        summary,
    }
}

pub(super) async fn detail(host: &dyn Host, repo: &Repository, number: u64) -> Result<PullDetail> {
    let path = item(repo, number);
    let m = host.request("GET", &path, None).await?;
    ensure!(m["iid"].as_u64() == Some(number), "Merge request not found");
    let paths = [
        project(repo),
        format!("{path}/discussions?per_page=100"),
        format!("{path}/approvals"),
        format!("{path}/commits?per_page=100"),
    ];
    let (project_info, viewer, discussions, approvals, commits) = tokio::join!(
        host.request("GET", &paths[0], None),
        host.request("GET", "user", None),
        host.request("GET", &paths[1], None),
        host.request("GET", &paths[2], None),
        host.request("GET", &paths[3], None),
    );
    let jobs = match m["head_pipeline"]["id"].as_u64() {
        Some(id) => host
            .request(
                "GET",
                &format!("{}/pipelines/{id}/jobs?per_page=100", project(repo)),
                None,
            )
            .await
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default(),
        None => Vec::new(),
    };
    Ok(map_detail(
        repo,
        &m,
        &project_info.unwrap_or_default(),
        viewer.ok().and_then(|v| opt_str(&v["username"])),
        &discussions?.as_array().cloned().unwrap_or_default(),
        &approvals.unwrap_or_default(),
        &commits?.as_array().cloned().unwrap_or_default(),
        &jobs,
    ))
}

fn map_diff(d: &Value) -> PullFile {
    let path = str_of(&d["new_path"]);
    let old = opt_str(&d["old_path"]).filter(|o| o != &path);
    let status = if d["new_file"] == true {
        "added"
    } else if d["deleted_file"] == true {
        "removed"
    } else if d["renamed_file"] == true {
        "renamed"
    } else {
        "modified"
    };
    let hunks = d["diff"].as_str();
    let patch = unified_patch(&path, old.as_deref(), status, hunks);
    let (mut additions, mut deletions) = (0, 0);
    for line in hunks.unwrap_or("").lines() {
        if line.starts_with('+') {
            additions += 1;
        } else if line.starts_with('-') {
            deletions += 1;
        }
    }
    PullFile {
        digest: digest(patch.as_deref().unwrap_or(&path)),
        binary: hunks.is_some_and(|h| h.starts_with("Binary files")),
        additions,
        deletions,
        viewed: "unviewed".into(),
        old_path: old,
        status: status.into(),
        path,
        patch,
    }
}

pub(super) async fn files(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
) -> Result<Vec<PullFile>> {
    let mut files = Vec::new();
    for page in 1..=5 {
        let value = match host
            .request(
                "GET",
                &format!("{}/diffs?per_page=100&page={page}", item(repo, number)),
                None,
            )
            .await
        {
            Ok(value) => value,
            // GitLab before 15.7 has only the single `changes` response.
            Err(_) if page == 1 => {
                let changes = host
                    .request("GET", &format!("{}/changes", item(repo, number)), None)
                    .await?;
                return Ok(changes["changes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(map_diff)
                    .collect());
            }
            Err(error) => return Err(error),
        };
        let batch = value.as_array().cloned().unwrap_or_default();
        files.extend(batch.iter().map(map_diff));
        if batch.len() < 100 {
            break;
        }
    }
    Ok(files)
}

/// Old and new line numbers for a line the reviewer picked on one side. GitLab
/// needs both for an unchanged line, and only one for an added or removed one.
pub(super) fn line_pair(patch: &str, side: &str, line: u64) -> Option<(Option<u64>, Option<u64>)> {
    let (mut old, mut new) = (0u64, 0u64);
    let mut in_hunk = false;
    for text in patch.lines() {
        if let Some(rest) = text.strip_prefix("@@ -") {
            let mut parts = rest.split(' ');
            old = parts.next()?.split(',').next()?.parse().ok()?;
            new = parts
                .next()?
                .trim_start_matches('+')
                .split(',')
                .next()?
                .parse()
                .ok()?;
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            continue;
        }
        match text.chars().next() {
            Some('+') => {
                if side == "right" && new == line {
                    return Some((None, Some(new)));
                }
                new += 1;
            }
            Some('-') => {
                if side == "left" && old == line {
                    return Some((Some(old), None));
                }
                old += 1;
            }
            Some('\\') => {}
            _ => {
                if (side == "right" && new == line) || (side == "left" && old == line) {
                    return Some((Some(old), Some(new)));
                }
                old += 1;
                new += 1;
            }
        }
    }
    None
}

pub(super) fn position(
    detail: &PullDetail,
    file: Option<&PullFile>,
    draft: &Draft,
) -> Result<Value> {
    let refs = &detail.extra["diffRefs"];
    ensure!(
        refs["head_sha"].is_string(),
        "Merge request diff refs unavailable; reload"
    );
    let path = draft.path.clone().unwrap_or_default();
    let old_path = file
        .and_then(|f| f.old_path.clone())
        .unwrap_or_else(|| path.clone());
    let mut position = json!({
        "base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"],
        "new_path": path, "old_path": old_path,
        "position_type": if draft.kind == "file" { "file" } else { "text" },
    });
    if draft.kind == "line" {
        let side = draft.side.as_deref().unwrap_or("right");
        let line = draft.line.unwrap_or(0);
        let (old, new) = file
            .and_then(|f| f.patch.as_deref())
            .and_then(|p| line_pair(p, side, line))
            .context("A commented line is no longer in the diff; load new commits")?;
        if let Some(old) = old {
            position["old_line"] = json!(old);
        }
        if let Some(new) = new {
            position["new_line"] = json!(new);
        }
    }
    Ok(position)
}

pub(super) async fn submit(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    input: &Submission<'_>,
) -> Result<Value> {
    let number = detail.summary.number;
    let path = item(repo, number);
    let files = if input.comments.iter().any(|d| d.kind == "line") {
        files(host, repo, number).await?
    } else {
        Vec::new()
    };
    // Build every position before posting anything, so a stale line refuses
    // the whole review instead of posting half of it.
    let mut posts = Vec::new();
    for draft in input.comments {
        let file = files.iter().find(|f| Some(&f.path) == draft.path.as_ref());
        let line = draft.line.unwrap_or(0);
        let body = match draft
            .start_line
            .filter(|s| draft.kind == "line" && *s != line)
        {
            Some(start) => format!("Lines {start}–{line}:\n\n{}", draft.body),
            None => draft.body.clone(),
        };
        posts.push(json!({"body":body,"position":position(detail, file, draft)?}));
    }
    for post in posts {
        host.request("POST", &format!("{path}/discussions"), Some(post))
            .await?;
    }
    if !input.body.trim().is_empty() {
        host.request(
            "POST",
            &format!("{path}/notes"),
            Some(json!({"body":input.body})),
        )
        .await?;
    }
    match input.event {
        // `sha` makes GitLab refuse the approval if the head moved (409).
        "approve" => {
            host.request(
                "POST",
                &format!("{path}/approve"),
                Some(json!({"sha":input.head})),
            )
            .await
        }
        "requestChanges" => {
            let viewer = detail.viewer.clone().context("GitLab user unknown")?;
            graphql(
                host,
                "mutation($projectPath:ID!,$iid:String!,$reviewers:[String!]!){mergeRequestSetReviewers(input:{projectPath:$projectPath,iid:$iid,reviewerUsernames:$reviewers,operationMode:APPEND}){errors}}",
                json!({"projectPath":full_path(repo),"iid":number.to_string(),"reviewers":[viewer]}),
            )
            .await?;
            graphql(
                host,
                "mutation($projectPath:ID!,$iid:String!){mergeRequestRequestChanges(input:{projectPath:$projectPath,iid:$iid}){errors}}",
                json!({"projectPath":full_path(repo),"iid":number.to_string()}),
            )
            .await
        }
        _ => Ok(json!({"ok":true})),
    }
}

pub(super) async fn reply(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
    thread: &str,
    body: &str,
) -> Result<Value> {
    host.request(
        "POST",
        &format!("{}/discussions/{}/notes", item(repo, number), enc(thread)),
        Some(json!({"body":body})),
    )
    .await
}

pub(super) async fn resolve(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
    thread: &str,
    resolved: bool,
) -> Result<Value> {
    host.request(
        "PUT",
        &format!("{}/discussions/{}", item(repo, number), enc(thread)),
        Some(json!({"resolved":resolved})),
    )
    .await
}

pub(super) async fn reviewers(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    add: &[String],
    remove: &[String],
) -> Result<Value> {
    let mut ids: Vec<(String, u64)> = detail.extra["reviewerIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| Some((r["login"].as_str()?.to_owned(), r["id"].as_u64()?)))
        .collect();
    ids.retain(|(login, _)| !remove.contains(login));
    for login in add {
        if ids.iter().any(|(l, _)| l == login) {
            continue;
        }
        let users = host
            .request("GET", &format!("users?username={}", enc(login)), None)
            .await?;
        let id = users[0]["id"]
            .as_u64()
            .with_context(|| format!("GitLab user {login} not found"))?;
        ids.push((login.clone(), id));
    }
    host.request(
        "PUT",
        &item(repo, detail.summary.number),
        Some(json!({"reviewer_ids":ids.iter().map(|(_, id)| *id).collect::<Vec<_>>()})),
    )
    .await
}

pub(super) async fn set_state(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    state: &str,
) -> Result<Value> {
    let title = &detail.summary.title;
    let body = match state {
        "close" => json!({"state_event":"close"}),
        "reopen" => json!({"state_event":"reopen"}),
        "draft" => json!({"title":format!("Draft: {title}")}),
        _ => {
            let mut t = title.as_str();
            for prefix in ["Draft:", "[Draft]", "(Draft)", "WIP:", "[WIP]"] {
                t = t.strip_prefix(prefix).unwrap_or(t).trim_start();
            }
            json!({"title":t})
        }
    };
    host.request("PUT", &item(repo, detail.summary.number), Some(body))
        .await
}

pub(super) async fn rerun(host: &dyn Host, repo: &Repository, token: &str) -> Result<Value> {
    let id = token
        .strip_prefix("pipeline:")
        .and_then(|id| id.parse::<u64>().ok())
        .context("Invalid rerun token")?;
    host.request(
        "POST",
        &format!("{}/pipelines/{id}/retry", project(repo)),
        Some(json!({})),
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
    let mut body = json!({"sha":input.expected_head,"squash":input.method=="squash"});
    if let Some((title, text)) = message {
        let text = if text.trim().is_empty() { title } else { text };
        body[if input.method == "squash" {
            "squash_commit_message"
        } else {
            "merge_commit_message"
        }] = json!(text);
    }
    host.request(
        "PUT",
        &format!("{}/merge", item(repo, detail.summary.number)),
        Some(body),
    )
    .await
}

pub(super) async fn auto_merge(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    input: &MergeInput<'_>,
    enable: bool,
) -> Result<Value> {
    let path = item(repo, detail.summary.number);
    if !enable {
        return host
            .request(
                "POST",
                &format!("{path}/cancel_merge_when_pipeline_succeeds"),
                Some(json!({})),
            )
            .await;
    }
    host.request(
        "PUT",
        &format!("{path}/merge"),
        Some(json!({"sha":input.expected_head,"squash":input.method=="squash","merge_when_pipeline_succeeds":true,"auto_merge":true})),
    )
    .await
}

pub(super) async fn merge_message(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    method: &str,
) -> Result<(String, String)> {
    let data = graphql(
        host,
        "query($fullPath:ID!,$iid:String!){project(fullPath:$fullPath){mergeRequest(iid:$iid){title diffHeadSha defaultMergeCommitMessage defaultSquashCommitMessage}}}",
        json!({"fullPath":full_path(repo),"iid":detail.summary.number.to_string()}),
    )
    .await?;
    let m = &data["project"]["mergeRequest"];
    guard_head(&detail.summary.head_sha, &str_of(&m["diffHeadSha"]))?;
    let text = str_of(if method == "squash" {
        &m["defaultSquashCommitMessage"]
    } else {
        &m["defaultMergeCommitMessage"]
    });
    let title = text.lines().next().unwrap_or("").to_owned();
    Ok((
        if title.is_empty() {
            str_of(&m["title"])
        } else {
            title
        },
        text,
    ))
}
