//! GitHub (and Enterprise): GraphQL for lists, detail, threads, viewed state and
//! reviews; REST where GraphQL has no equivalent (files with patches, reviewers,
//! merge with an atomic `sha`, workflow reruns).
use super::*;
use crate::source_control::hosts::Host;

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

const SUMMARY_FIELDS: &str = "number title url isDraft state createdAt updatedAt headRefName baseRefName headRefOid additions deletions changedFiles author{login avatarUrl} reviewDecision totalCommentsCount labels(first:10){nodes{name color}} reviewRequests(first:20){nodes{requestedReviewer{__typename ... on User{login} ... on Team{slug name}}}} commits(last:1){nodes{commit{statusCheckRollup{state}}}}";

/// GitHub stacks exist on github.com; Enterprise servers may not know the fields.
pub(super) fn stack_fields(repo: &Repository) -> &'static str {
    if repo.host == "github.com" {
        " stack{number size} stackEntry{position}"
    } else {
        ""
    }
}

pub(super) fn state(value: &Value) -> String {
    match value.as_str().unwrap_or("") {
        "MERGED" => "merged",
        "CLOSED" => "closed",
        _ => "open",
    }
    .into()
}
fn rollup(value: &Value) -> Option<String> {
    Some(
        match value.as_str() {
            Some("SUCCESS") => "passed",
            Some("FAILURE" | "ERROR") => "failed",
            Some("PENDING" | "EXPECTED") => "pending",
            _ => "none",
        }
        .into(),
    )
}
fn decision(value: &Value) -> Option<String> {
    match value.as_str() {
        Some("APPROVED") => Some("approved".into()),
        Some("CHANGES_REQUESTED") => Some("changesRequested".into()),
        Some("REVIEW_REQUIRED") => Some("reviewRequired".into()),
        _ => None,
    }
}
fn review_state(value: &Value) -> String {
    match value.as_str().unwrap_or("") {
        "APPROVED" => "approved",
        "CHANGES_REQUESTED" => "changesRequested",
        "DISMISSED" => "dismissed",
        "PENDING" => "pending",
        _ => "commented",
    }
    .into()
}
fn requested(node: &Value) -> Option<(String, bool)> {
    let reviewer = &node["requestedReviewer"];
    if reviewer["__typename"] == "Team" {
        opt_str(&reviewer["slug"]).map(|s| (s, true))
    } else {
        opt_str(&reviewer["login"]).map(|s| (s, false))
    }
}

pub(super) fn summary(repo: &Repository, n: &Value) -> PullSummary {
    PullSummary {
        stack: n["stack"]["number"].as_u64().map(|number| StackRef {
            id: number.to_string(),
            position: n["stackEntry"]["position"].as_u64().unwrap_or(0),
            size: n["stack"]["size"].as_u64().unwrap_or(0),
            native: true,
        }),
        url: str_of(&n["url"]),
        number: n["number"].as_u64().unwrap_or(0),
        title: str_of(&n["title"]),
        state: state(&n["state"]),
        draft: n["isDraft"] == true,
        author: person(
            n["author"]["login"].as_str(),
            n["author"]["avatarUrl"].as_str(),
        ),
        repository: repo.clone(),
        head_ref: str_of(&n["headRefName"]),
        base_ref: str_of(&n["baseRefName"]),
        head_sha: str_of(&n["headRefOid"]),
        created_at: str_of(&n["createdAt"]),
        updated_at: str_of(&n["updatedAt"]),
        checks: rollup(&n["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"]),
        review_decision: decision(&n["reviewDecision"]),
        requested_reviewers: n["reviewRequests"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(requested)
            .map(|(login, _)| login)
            .collect(),
        additions: n["additions"].as_u64(),
        deletions: n["deletions"].as_u64(),
        changed_files: n["changedFiles"].as_u64(),
        comments: n["totalCommentsCount"].as_u64(),
        labels: n["labels"]["nodes"]
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
    let states = match state {
        "open" => json!(["OPEN"]),
        "closed" => json!(["CLOSED"]),
        "merged" => json!(["MERGED"]),
        _ => json!(["OPEN", "CLOSED", "MERGED"]),
    };
    let query = format!("query($owner:String!,$name:String!,$states:[PullRequestState!],$first:Int!){{repository(owner:$owner,name:$name){{pullRequests(states:$states,first:$first,orderBy:{{field:UPDATED_AT,direction:DESC}}){{nodes{{{SUMMARY_FIELDS}{}}}}}}}}}", stack_fields(repo));
    let data = graphql(
        host,
        &query,
        json!({"owner":repo.owner,"name":repo.name,"states":states,"first":50}),
    )
    .await?;
    Ok(data["repository"]["pullRequests"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|n| summary(repo, n))
        .collect())
}

const DETAIL: &str = "query($owner:String!,$name:String!,$number:Int!){viewer{login} repository(owner:$owner,name:$name){mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed autoMergeAllowed viewerPermission pullRequest(number:$number){id body baseRefOid mergeable mergeStateStatus viewerCanUpdate autoMergeRequest{mergeMethod} latestReviews(first:30){nodes{author{login} state}} comments(first:100){nodes{id author{login avatarUrl} body createdAt url}} reviews(first:100){nodes{id author{login avatarUrl} state body submittedAt url commit{oid}}} reviewThreads(first:100){nodes{id isResolved isOutdated path line originalLine startLine originalStartLine diffSide subjectType viewerCanResolve viewerCanUnresolve viewerCanReply comments(first:50){nodes{id author{login avatarUrl} body createdAt url}}}} allCommits:commits(last:100){nodes{commit{oid message committedDate url author{name user{login}}}}} lastCommit:commits(last:1){nodes{commit{statusCheckRollup{contexts(first:100){nodes{__typename ... on CheckRun{databaseId name status conclusion detailsUrl startedAt completedAt checkSuite{workflowRun{databaseId}}} ... on StatusContext{context state targetUrl description createdAt}}}}}}} SUMMARY}}}";

fn check_run(n: &Value) -> Option<Check> {
    if n["__typename"] == "StatusContext" {
        let state = match n["state"].as_str()? {
            "SUCCESS" => "passed",
            "FAILURE" | "ERROR" => "failed",
            _ => "pending",
        };
        return Some(Check {
            id: format!("status:{}", str_of(&n["context"])),
            name: str_of(&n["context"]),
            state: state.into(),
            url: opt_str(&n["targetUrl"]).filter(|u| u.starts_with("https://")),
            description: opt_str(&n["description"]),
            started_at: opt_str(&n["createdAt"]),
            completed_at: None,
            rerun: None,
        });
    }
    let state = if n["status"] != "COMPLETED" {
        "pending"
    } else {
        match n["conclusion"].as_str().unwrap_or("") {
            "SUCCESS" => "passed",
            "SKIPPED" => "skipped",
            "NEUTRAL" => "neutral",
            "CANCELLED" => "cancelled",
            _ => "failed",
        }
    };
    let run = n["checkSuite"]["workflowRun"]["databaseId"].as_u64();
    Some(Check {
        id: format!("check:{}", n["databaseId"].as_u64().unwrap_or(0)),
        name: str_of(&n["name"]),
        state: state.into(),
        url: opt_str(&n["detailsUrl"]).filter(|u| u.starts_with("https://")),
        description: None,
        started_at: opt_str(&n["startedAt"]),
        completed_at: opt_str(&n["completedAt"]),
        rerun: run
            .filter(|_| matches!(state, "failed" | "cancelled"))
            .map(|id| format!("run:{id}")),
    })
}

pub(super) fn map_detail(repo: &Repository, data: &Value) -> Result<PullDetail> {
    let r = &data["repository"];
    let p = &r["pullRequest"];
    ensure!(p.is_object(), "PR not found");
    let summary = summary(repo, p);
    let mut reviewers: Vec<Reviewer> = Vec::new();
    for n in p["latestReviews"]["nodes"].as_array().into_iter().flatten() {
        if let Some(login) = opt_str(&n["author"]["login"]) {
            reviewers.push(Reviewer {
                login,
                state: review_state(&n["state"]),
                team: false,
            });
        }
    }
    for n in p["reviewRequests"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some((login, team)) = requested(n) {
            reviewers.retain(|r| r.login != login);
            reviewers.push(Reviewer {
                login,
                state: "requested".into(),
                team,
            });
        }
    }
    let mut timeline = Vec::new();
    for n in p["comments"]["nodes"].as_array().into_iter().flatten() {
        timeline.push(TimelineEntry {
            kind: "comment".into(),
            id: str_of(&n["id"]),
            author: person(
                n["author"]["login"].as_str(),
                n["author"]["avatarUrl"].as_str(),
            ),
            body: str_of(&n["body"]),
            created_at: str_of(&n["createdAt"]),
            url: opt_str(&n["url"]),
            state: None,
            commit: None,
        });
    }
    for n in p["reviews"]["nodes"].as_array().into_iter().flatten() {
        let state = review_state(&n["state"]);
        // A COMMENTED review without text only carries line comments, which the
        // threads show; listing it again would be noise.
        if state == "pending" || (state == "commented" && str_of(&n["body"]).trim().is_empty()) {
            continue;
        }
        timeline.push(TimelineEntry {
            kind: "review".into(),
            id: str_of(&n["id"]),
            author: person(
                n["author"]["login"].as_str(),
                n["author"]["avatarUrl"].as_str(),
            ),
            body: str_of(&n["body"]),
            created_at: str_of(&n["submittedAt"]),
            url: opt_str(&n["url"]),
            state: Some(state),
            commit: opt_str(&n["commit"]["oid"]),
        });
    }
    timeline.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let threads = p["reviewThreads"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|n| ReviewThread {
            id: str_of(&n["id"]),
            path: str_of(&n["path"]),
            line: n["line"].as_u64().or(n["originalLine"].as_u64()),
            start_line: n["startLine"].as_u64().or(n["originalStartLine"].as_u64()),
            side: if n["diffSide"] == "LEFT" {
                "left"
            } else {
                "right"
            }
            .into(),
            subject: if n["subjectType"] == "FILE" {
                "file"
            } else {
                "line"
            }
            .into(),
            resolved: n["isResolved"] == true,
            outdated: n["isOutdated"] == true,
            can_resolve: n["viewerCanResolve"] == true || n["viewerCanUnresolve"] == true,
            can_reply: n["viewerCanReply"] == true,
            comments: n["comments"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| ThreadComment {
                    id: str_of(&c["id"]),
                    author: person(
                        c["author"]["login"].as_str(),
                        c["author"]["avatarUrl"].as_str(),
                    ),
                    body: str_of(&c["body"]),
                    created_at: str_of(&c["createdAt"]),
                    url: opt_str(&c["url"]),
                })
                .collect(),
        })
        .collect();
    let commits = p["allCommits"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|n| {
            let c = &n["commit"];
            Commit {
                sha: str_of(&c["oid"]),
                message: str_of(&c["message"]),
                author: str_of(&c["author"]["name"]),
                login: opt_str(&c["author"]["user"]["login"]),
                date: str_of(&c["committedDate"]),
                url: opt_str(&c["url"]),
            }
        })
        .collect();
    let check_runs = p["lastCommit"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]
        ["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(check_run)
        .collect();
    let mut merge_methods = Vec::new();
    for (key, method) in [
        ("squashMergeAllowed", "squash"),
        ("mergeCommitAllowed", "merge"),
        ("rebaseMergeAllowed", "rebase"),
    ] {
        if r[key] == true {
            merge_methods.push(method.to_owned());
        }
    }
    let writer = matches!(
        r["viewerPermission"].as_str(),
        Some("ADMIN" | "MAINTAIN" | "WRITE")
    );
    let open = summary.state == "open";
    Ok(PullDetail {
        body: str_of(&p["body"]),
        node_id: opt_str(&p["id"]),
        base_sha: opt_str(&p["baseRefOid"]),
        mergeable: match p["mergeable"].as_str() {
            Some("MERGEABLE") => Some(true),
            Some("CONFLICTING") => Some(false),
            _ => None,
        },
        merge_state: Some(
            match p["mergeStateStatus"].as_str().unwrap_or("") {
                "CLEAN" | "HAS_HOOKS" => "clean",
                "DIRTY" => "conflicts",
                "BLOCKED" => "blocked",
                "BEHIND" => "behind",
                "UNSTABLE" => "unstable",
                "DRAFT" => "draft",
                _ => "unknown",
            }
            .into(),
        ),
        auto_merge: opt_str(&p["autoMergeRequest"]["mergeMethod"]).map(|m| m.to_lowercase()),
        reviewers,
        timeline,
        threads,
        commits,
        check_runs,
        viewer: opt_str(&data["viewer"]["login"]),
        capabilities: Capabilities {
            submit: vec!["comment".into(), "approve".into(), "requestChanges".into()],
            line_comments: true,
            file_comments: true,
            reply: true,
            resolve: true,
            viewed_native: true,
            reviewers: writer && open,
            convert_to_draft: writer,
            ready: writer,
            close: p["viewerCanUpdate"] == true,
            reopen: p["viewerCanUpdate"] == true,
            auto_merge: writer && r["autoMergeAllowed"] == true,
            merge: writer,
            revert: writer,
            rerun: writer,
            remove_credits: true,
        },
        merge_methods,
        extra: Value::Null,
        summary,
    })
}

pub(super) async fn detail(host: &dyn Host, repo: &Repository, number: u64) -> Result<PullDetail> {
    let query = DETAIL.replace(
        "SUMMARY",
        &format!("{SUMMARY_FIELDS}{}", stack_fields(repo)),
    );
    let data = graphql(
        host,
        &query,
        json!({"owner":repo.owner,"name":repo.name,"number":number}),
    )
    .await?;
    map_detail(repo, &data)
}

pub(super) fn map_file(f: &Value) -> PullFile {
    let path = str_of(&f["filename"]);
    let old = opt_str(&f["previous_filename"]);
    let status = file_status(f["status"].as_str().unwrap_or(""));
    let hunks = f["patch"].as_str();
    let patch = unified_patch(&path, old.as_deref(), &status, hunks);
    PullFile {
        digest: digest(&format!(
            "{}\n{}",
            str_of(&f["sha"]),
            patch.as_deref().unwrap_or("")
        )),
        binary: hunks.is_none() && f["changes"].as_u64() == Some(0),
        additions: f["additions"].as_u64().unwrap_or(0),
        deletions: f["deletions"].as_u64().unwrap_or(0),
        viewed: "unviewed".into(),
        old_path: old,
        status,
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
    // Three pages of 100 cover what the host lists before it truncates (3,000 files
    // through REST is rare in review; the summary keeps the real count).
    for page in 1..=3 {
        let value = host
            .request(
                "GET",
                &format!(
                    "repos/{}/{}/pulls/{number}/files?per_page=100&page={page}",
                    enc(&repo.owner),
                    enc(&repo.name)
                ),
                None,
            )
            .await?;
        let batch = value.as_array().cloned().unwrap_or_default();
        let done = batch.len() < 100;
        files.extend(batch.iter().map(map_file));
        if done {
            break;
        }
    }
    Ok(files)
}

fn side(value: Option<&str>) -> &'static str {
    if value == Some("left") {
        "LEFT"
    } else {
        "RIGHT"
    }
}

pub(super) fn review_input(detail: &PullDetail, input: &Submission) -> Result<Value> {
    let threads = input
        .comments
        .iter()
        .map(|d| {
            let mut thread = json!({"path":d.path,"body":d.body});
            if d.kind == "file" {
                thread["subjectType"] = json!("FILE");
            } else {
                thread["line"] = json!(d.line);
                thread["side"] = json!(side(d.side.as_deref()));
                if let Some(start) = d.start_line.filter(|s| Some(*s) != d.line) {
                    thread["startLine"] = json!(start);
                    thread["startSide"] = json!(side(d.side.as_deref()));
                }
            }
            thread
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "pullRequestId": detail.node_id.as_deref().context("PR node id missing; reload")?,
        "commitOID": input.head,
        "body": input.body,
        "event": match input.event { "approve" => "APPROVE", "requestChanges" => "REQUEST_CHANGES", _ => "COMMENT" },
        "threads": threads,
    }))
}

pub(super) async fn submit(
    host: &dyn Host,
    _repo: &Repository,
    detail: &PullDetail,
    input: &Submission<'_>,
) -> Result<Value> {
    let data = graphql(
        host,
        "mutation($input:AddPullRequestReviewInput!){addPullRequestReview(input:$input){pullRequestReview{id url state}}}",
        json!({"input":review_input(detail, input)?}),
    )
    .await?;
    Ok(data["addPullRequestReview"]["pullRequestReview"].clone())
}

pub(super) async fn reply(host: &dyn Host, thread: &str, body: &str) -> Result<Value> {
    let data = graphql(
        host,
        "mutation($thread:ID!,$body:String!){addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$thread,body:$body}){comment{id url}}}",
        json!({"thread":thread,"body":body}),
    )
    .await?;
    Ok(data["addPullRequestReviewThreadReply"]["comment"].clone())
}

pub(super) async fn resolve(host: &dyn Host, thread: &str, resolved: bool) -> Result<Value> {
    let mutation = if resolved {
        "resolveReviewThread"
    } else {
        "unresolveReviewThread"
    };
    let data = graphql(
        host,
        &format!("mutation($thread:ID!){{{mutation}(input:{{threadId:$thread}}){{thread{{id isResolved}}}}}}"),
        json!({"thread":thread}),
    )
    .await?;
    Ok(data[mutation]["thread"].clone())
}

pub(super) async fn set_viewed(
    host: &dyn Host,
    detail: &PullDetail,
    path: &str,
    viewed: bool,
) -> Result<()> {
    let mutation = if viewed {
        "markFileAsViewed"
    } else {
        "unmarkFileAsViewed"
    };
    graphql(
        host,
        &format!("mutation($id:ID!,$path:String!){{{mutation}(input:{{pullRequestId:$id,path:$path}}){{clientMutationId}}}}"),
        json!({"id":detail.node_id,"path":path}),
    )
    .await?;
    Ok(())
}

pub(super) async fn reviewers(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
    add: &[String],
    remove: &[String],
) -> Result<Value> {
    let path = format!(
        "repos/{}/{}/pulls/{number}/requested_reviewers",
        enc(&repo.owner),
        enc(&repo.name)
    );
    // `org/team` names a team; anything else is a user login.
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
    let number = detail.summary.number;
    match state {
        "close" | "reopen" => {
            host.request(
                "PATCH",
                &format!(
                    "repos/{}/{}/pulls/{number}",
                    enc(&repo.owner),
                    enc(&repo.name)
                ),
                Some(json!({"state":if state=="close"{"closed"}else{"open"}})),
            )
            .await
        }
        _ => {
            let mutation = if state == "draft" {
                "convertPullRequestToDraft"
            } else {
                "markPullRequestReadyForReview"
            };
            graphql(
                host,
                &format!("mutation($id:ID!){{{mutation}(input:{{pullRequestId:$id}}){{pullRequest{{isDraft}}}}}}"),
                json!({"id":detail.node_id}),
            )
            .await
        }
    }
}

pub(super) async fn rerun(host: &dyn Host, repo: &Repository, token: &str) -> Result<Value> {
    let run = token
        .strip_prefix("run:")
        .and_then(|id| id.parse::<u64>().ok())
        .context("Invalid rerun token")?;
    host.request(
        "POST",
        &format!(
            "repos/{}/{}/actions/runs/{run}/rerun-failed-jobs",
            enc(&repo.owner),
            enc(&repo.name)
        ),
        Some(json!({})),
    )
    .await
}

pub(super) async fn revert(host: &dyn Host, detail: &PullDetail) -> Result<Value> {
    let data = graphql(
        host,
        "mutation($id:ID!){revertPullRequest(input:{pullRequestId:$id}){revertPullRequest{number url}}}",
        json!({"id":detail.node_id}),
    )
    .await?;
    Ok(data["revertPullRequest"]["revertPullRequest"].clone())
}

pub(super) async fn merge(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    input: &MergeInput<'_>,
    message: Option<(String, String)>,
) -> Result<Value> {
    // `sha` makes GitHub itself refuse when the head moved after our check.
    let mut body = json!({"merge_method":input.method,"sha":input.expected_head});
    if let Some((title, message)) = message {
        body["commit_title"] = json!(title);
        body["commit_message"] = json!(message);
    }
    host.request(
        "PUT",
        &format!(
            "repos/{}/{}/pulls/{}/merge",
            enc(&repo.owner),
            enc(&repo.name),
            detail.summary.number
        ),
        Some(body),
    )
    .await
}

pub(super) async fn auto_merge(
    host: &dyn Host,
    detail: &PullDetail,
    input: &MergeInput<'_>,
    enable: bool,
    message: Option<(String, String)>,
) -> Result<Value> {
    if !enable {
        return graphql(
            host,
            "mutation($id:ID!){disablePullRequestAutoMerge(input:{pullRequestId:$id}){pullRequest{number}}}",
            json!({"id":detail.node_id}),
        )
        .await;
    }
    let mut variables = json!({"id":detail.node_id,"method":input.method.to_uppercase(),"head":input.expected_head,"headline":null,"body":null});
    if let Some((title, body)) = message {
        variables["headline"] = json!(title);
        variables["body"] = json!(body);
    }
    graphql(
        host,
        "mutation($id:ID!,$method:PullRequestMergeMethod!,$head:GitObjectID!,$headline:String,$body:String){enablePullRequestAutoMerge(input:{pullRequestId:$id,mergeMethod:$method,expectedHeadOid:$head,commitHeadline:$headline,commitBody:$body}){pullRequest{number}}}",
        variables,
    )
    .await
}

pub(super) async fn merge_message(
    host: &dyn Host,
    repo: &Repository,
    detail: &PullDetail,
    method: &str,
) -> Result<(String, String)> {
    let kind = if method == "squash" {
        "SQUASH"
    } else {
        "MERGE"
    };
    let data = graphql(
        host,
        &format!("query($owner:String!,$name:String!,$number:Int!){{repository(owner:$owner,name:$name){{pullRequest(number:$number){{headRefOid viewerMergeHeadlineText(mergeType:{kind}) viewerMergeBodyText(mergeType:{kind})}}}}}}"),
        json!({"owner":repo.owner,"name":repo.name,"number":detail.summary.number}),
    )
    .await?;
    let p = &data["repository"]["pullRequest"];
    // Read after the guard: a push since then must fail the merge, not merge
    // commits whose credits this text does not cover.
    guard_head(&detail.summary.head_sha, &str_of(&p["headRefOid"]))?;
    Ok((
        str_of(&p["viewerMergeHeadlineText"]),
        str_of(&p["viewerMergeBodyText"]),
    ))
}

/// Viewed state per file, GitHub's own (`DISMISSED` is its "changed since viewed").
pub(super) async fn viewed(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
) -> Result<Vec<(String, String)>> {
    let mut states = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..5 {
        let data = graphql(
            host,
            "query($owner:String!,$name:String!,$number:Int!,$after:String){repository(owner:$owner,name:$name){pullRequest(number:$number){files(first:100,after:$after){pageInfo{hasNextPage endCursor} nodes{path viewerViewedState}}}}}",
            json!({"owner":repo.owner,"name":repo.name,"number":number,"after":after}),
        )
        .await?;
        let files = &data["repository"]["pullRequest"]["files"];
        for n in files["nodes"].as_array().into_iter().flatten() {
            states.push((
                str_of(&n["path"]),
                match n["viewerViewedState"].as_str() {
                    Some("VIEWED") => "viewed",
                    Some("DISMISSED") => "changed",
                    _ => "unviewed",
                }
                .to_owned(),
            ));
        }
        if files["pageInfo"]["hasNextPage"] != true {
            break;
        }
        after = opt_str(&files["pageInfo"]["endCursor"]);
    }
    Ok(states)
}
