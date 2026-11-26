//! Review behaviour against recorded host responses (GitHub recordings are
//! read-only captures of a public PR with logins and e-mail replaced) and hand
//! shaped GitLab/Forgejo/Bitbucket/Azure responses from their API references.
use super::*;
use crate::source_control::hosts::Operation;
use async_trait::async_trait;
use std::sync::Mutex;

/// A host that answers from fixtures and records every request it receives.
struct FixtureHost {
    kind: HostKind,
    answers: Vec<(&'static str, String, Value)>,
    texts: Vec<(String, String)>,
    log: Mutex<Vec<(String, String, Option<Value>)>>,
}
impl FixtureHost {
    fn new(kind: HostKind) -> Self {
        Self {
            kind,
            answers: Vec::new(),
            texts: Vec::new(),
            log: Mutex::new(Vec::new()),
        }
    }
    fn answer(mut self, method: &'static str, prefix: &str, value: Value) -> Self {
        self.answers.push((method, prefix.into(), value));
        self
    }
    #[allow(dead_code)]
    fn text(mut self, prefix: &str, value: &str) -> Self {
        self.texts.push((prefix.into(), value.into()));
        self
    }
    fn writes(&self) -> Vec<(String, String, Option<Value>)> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, p, _)| m != "GET" && p != "graphql")
            .cloned()
            .collect()
    }
    fn requests(&self) -> Vec<(String, String, Option<Value>)> {
        self.log.lock().unwrap().clone()
    }
}
#[async_trait]
impl Host for FixtureHost {
    fn identity_path(&self) -> String {
        "user".into()
    }
    fn operation(
        &self,
        action: &str,
        _: bool,
        number: Option<u64>,
        _: &Value,
    ) -> Result<Operation> {
        let path = match (self.kind, action) {
            (HostKind::Github | HostKind::Forgejo, "show") => {
                format!("repos/o/r/pulls/{}", number.unwrap_or(0))
            }
            (HostKind::Gitlab, "show") => {
                format!("projects/o%2Fr/merge_requests/{}", number.unwrap_or(0))
            }
            _ => format!("op/{action}"),
        };
        Ok(Operation {
            method: "GET".into(),
            path,
            body: None,
        })
    }
    async fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        self.log
            .lock()
            .unwrap()
            .push((method.into(), path.into(), body.clone()));
        let query = body
            .as_ref()
            .and_then(|b| b["query"].as_str())
            .unwrap_or("");
        for (m, prefix, value) in &self.answers {
            let matches = if *m == "GQL" {
                path == "graphql" && query.contains(prefix.as_str())
            } else {
                *m == method && path.starts_with(prefix.as_str())
            };
            if matches {
                return Ok(value.clone());
            }
        }
        if method == "GET" {
            bail!("no fixture for {method} {path}")
        }
        Ok(json!({"ok":true}))
    }
    async fn request_text(&self, path: &str) -> Result<String> {
        self.texts
            .iter()
            .find(|(p, _)| path.starts_with(p.as_str()))
            .map(|(_, t)| t.clone())
            .context("no text fixture")
    }
}

fn repo(kind: HostKind) -> Repository {
    Repository {
        host: match kind {
            HostKind::Github => "github.com",
            HostKind::Gitlab => "gitlab.com",
            HostKind::Bitbucket => "bitbucket.org",
            HostKind::Azure => "dev.azure.com",
            HostKind::Forgejo => "127.0.0.1:19173",
        }
        .into(),
        owner: if kind == HostKind::Azure {
            "org/project/_git".into()
        } else {
            "o".into()
        },
        name: "r".into(),
        kind,
    }
}

fn draft(
    kind: &str,
    path: &str,
    side: &str,
    line: Option<u64>,
    start: Option<u64>,
    body: &str,
) -> Draft {
    Draft {
        id: format!("{path}:{line:?}"),
        kind: kind.into(),
        path: Some(path.into()),
        side: Some(side.into()),
        line,
        start_line: start,
        body: body.into(),
        head_sha: None,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

fn github_detail() -> PullDetail {
    let recorded: Value =
        serde_json::from_str(include_str!("fixtures/github-detail.json")).unwrap();
    github::map_detail(&repo(HostKind::Github), &recorded["data"]).unwrap()
}

#[test]
fn github_recorded_detail_maps_reviews_threads_checks_and_commits() {
    let d = github_detail();
    assert_eq!(d.summary.number, 15100);
    assert_eq!(d.summary.state, "merged");
    assert_eq!(
        d.summary.head_sha,
        "fe9ea5c35b64183e94ba4cdfd2cb679cc42b2a19"
    );
    assert_eq!(d.viewer.as_deref(), Some("fixture-user-2"));
    // Recorded threads: resolved, outdated, anchored with a side.
    assert_eq!(d.threads.len(), 3);
    assert!(d.threads.iter().all(|t| t.resolved));
    assert!(
        d.threads[0].outdated && d.threads[0].line == Some(175) && d.threads[0].side == "right"
    );
    assert!(d.threads[0].comments.len() >= 2 && d.threads[0].can_reply);
    // Empty COMMENTED reviews carry only line comments and stay out of the timeline.
    assert!(d
        .timeline
        .iter()
        .any(|e| e.state.as_deref() == Some("changesRequested")));
    assert!(d.timeline.iter().all(|e| e.kind == "comment"
        || !(e.state.as_deref() == Some("commented") && e.body.trim().is_empty())));
    assert!(d
        .timeline
        .windows(2)
        .all(|w| w[0].created_at <= w[1].created_at));
    assert!(d.reviewers.iter().any(|r| r.state == "approved"));
    assert_eq!(d.commits.len(), 3);
    assert!(d
        .check_runs
        .iter()
        .all(|c| c.state == "passed" && c.rerun.is_none()));
    // A read-only viewer gets no write capabilities, but can still review.
    assert!(!d.capabilities.merge && !d.capabilities.reviewers);
    assert_eq!(d.capabilities.submit.len(), 3);
    assert!(d.capabilities.viewed_native);
}

#[test]
fn github_recorded_files_become_unified_patches() {
    let files: Value = serde_json::from_str(include_str!("fixtures/github-files.json")).unwrap();
    let files: Vec<PullFile> = files
        .as_array()
        .unwrap()
        .iter()
        .map(github::map_file)
        .collect();
    assert_eq!(files[0].status, "added");
    let patch = files[0].patch.as_deref().unwrap();
    assert!(patch.starts_with("diff --git a/internal/authflow/revoke.go b/internal/authflow/revoke.go\nnew file mode 100644\n--- /dev/null\n+++ b/internal/authflow/revoke.go\n@@ -0,0 +1,75 @@"));
    assert_ne!(files[0].digest, files[1].digest);
}

#[tokio::test]
async fn github_submit_sends_one_review_with_line_range_and_file_threads() {
    let mut d = github_detail();
    d.summary.state = "open".into();
    let host = FixtureHost::new(HostKind::Github).answer(
        "GQL",
        "addPullRequestReview",
        json!({"data":{"addPullRequestReview":{"pullRequestReview":{"id":"R1","url":"https://github.com/o/r/pull/1#r","state":"CHANGES_REQUESTED"}}}}),
    );
    let drafts = vec![
        draft(
            "line",
            "src/a.rs",
            "right",
            Some(12),
            Some(10),
            "Why three passes?",
        ),
        draft(
            "line",
            "src/a.rs",
            "left",
            Some(4),
            None,
            "This removal drops the retry",
        ),
        draft(
            "file",
            "docs/x.md",
            "right",
            None,
            None,
            "Needs a Chinese version",
        ),
    ];
    let head = d.summary.head_sha.clone();
    let review = submit(
        &host,
        &repo(HostKind::Github),
        &d,
        &Submission {
            event: "requestChanges",
            body: "Two questions",
            comments: &drafts,
            head: &head,
        },
    )
    .await
    .unwrap();
    assert_eq!(review["state"], "CHANGES_REQUESTED");
    let requests = host.requests();
    assert_eq!(requests.len(), 1, "one atomic review call");
    let input = &requests[0].2.as_ref().unwrap()["variables"]["input"];
    assert_eq!(input["event"], "REQUEST_CHANGES");
    assert_eq!(input["commitOID"], head);
    assert_eq!(input["threads"][0]["line"], 12);
    assert_eq!(input["threads"][0]["startLine"], 10);
    assert_eq!(input["threads"][0]["side"], "RIGHT");
    assert_eq!(input["threads"][1]["side"], "LEFT");
    assert!(input["threads"][1].get("startLine").is_none());
    assert_eq!(input["threads"][2]["subjectType"], "FILE");
    assert!(input["threads"][2].get("line").is_none());
}

#[tokio::test]
async fn submit_and_merge_refuse_a_moved_head_before_any_write() {
    let mut d = github_detail();
    d.summary.state = "open".into();
    d.capabilities.merge = true;
    d.merge_methods = vec!["squash".into()];
    let host = FixtureHost::new(HostKind::Github);
    let drafts = vec![draft("line", "a", "right", Some(1), None, "x")];
    let error = submit(
        &host,
        &repo(HostKind::Github),
        &d,
        &Submission {
            event: "comment",
            body: "",
            comments: &drafts,
            head: "stale",
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("head moved"));
    let error = merge(
        &host,
        &repo(HostKind::Github),
        &d,
        &MergeInput {
            method: "squash",
            expected_head: "stale",
            remove_credits: true,
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("head moved"));
    assert!(host.requests().is_empty(), "nothing sent to the host");
}

#[test]
fn submissions_are_checked_against_host_capabilities() {
    let caps = Capabilities {
        submit: vec!["comment".into(), "approve".into()],
        line_comments: true,
        ..Default::default()
    };
    let empty: Vec<Draft> = Vec::new();
    let comment = |event, body, comments| Submission {
        event,
        body,
        comments,
        head: "h",
    };
    assert!(validate_submission(&caps, &comment("comment", " ", &empty)).is_err());
    assert!(validate_submission(&caps, &comment("approve", "", &empty)).is_ok());
    assert!(validate_submission(&caps, &comment("requestChanges", "x", &empty)).is_err());
    let lines = vec![draft("line", "a", "right", None, None, "x")];
    assert!(validate_submission(&caps, &comment("comment", "", &lines)).is_err());
    let folded = fold_file_comments(
        &caps,
        "Summary",
        &[draft("file", "a.md", "right", None, None, "Typo")],
    );
    assert_eq!(folded, "Summary\n\n**a.md**\n\nTypo");
}

fn forgejo_detail() -> PullDetail {
    let pr = json!({"number":3,"title":"WIP: Parser","html_url":"http://127.0.0.1:19173/o/r/pulls/3","state":"open","merged":false,"user":{"login":"alice"},"head":{"ref":"feature","sha":"h1"},"base":{"ref":"main","sha":"b1"},"created_at":"2026-11-26T10:00:00Z","updated_at":"2026-11-26T11:00:00Z","requested_reviewers":[{"login":"bob"}],"mergeable":true,"body":"Adds a parser"});
    let reviews = vec![
        json!({"id":9,"user":{"login":"carol"},"state":"REQUEST_CHANGES","body":"Split it","commit_id":"h0","submitted_at":"2026-11-26T10:30:00Z","comments_count":2}),
    ];
    let comments = vec![
        json!({"id":21,"user":{"login":"carol"},"body":"Off by one","path":"src/p.rs","position":14,"original_position":0,"commit_id":"h0","created_at":"2026-11-26T10:30:00Z","resolver":null}),
        json!({"id":22,"user":{"login":"alice"},"body":"Fixed","path":"src/p.rs","position":14,"original_position":0,"commit_id":"h1","created_at":"2026-11-26T10:40:00Z","resolver":{"login":"alice"}}),
        json!({"id":23,"user":{"login":"carol"},"body":"Old code","path":"src/p.rs","position":0,"original_position":3,"commit_id":"h1","created_at":"2026-11-26T10:31:00Z"}),
    ];
    let status = json!({"state":"failure","statuses":[{"id":5,"status":"failure","context":"ci/test","target_url":"http://127.0.0.1:19173/o/r/actions/runs/1","created_at":"2026-11-26T10:05:00Z"}]});
    forgejo::map_detail(
        &repo(HostKind::Forgejo),
        &pr,
        &json!({"permissions":{"push":true},"allow_rebase":false}),
        Some("alice".into()),
        &reviews,
        &comments,
        &[
            json!({"id":1,"user":{"login":"bob"},"body":"LGTM soon","created_at":"2026-11-26T10:10:00Z"}),
        ],
        &[
            json!({"sha":"h1","commit":{"message":"Add parser\n\nCo-Authored-By: Codex <codex@openai.com>","author":{"name":"Alice","date":"2026-11-26T09:00:00Z"}}}),
        ],
        &status,
    )
}

#[test]
fn forgejo_groups_review_comments_into_conversations() {
    let d = forgejo_detail();
    assert!(d.summary.draft, "WIP: title marks a draft");
    assert_eq!(d.summary.checks.as_deref(), Some("failed"));
    assert_eq!(
        d.summary.review_decision.as_deref(),
        Some("changesRequested")
    );
    assert_eq!(d.threads.len(), 2);
    let right = d.threads.iter().find(|t| t.side == "right").unwrap();
    assert_eq!(
        (right.line, right.comments.len(), right.resolved),
        (Some(14), 2, true)
    );
    let left = d.threads.iter().find(|t| t.side == "left").unwrap();
    assert_eq!(left.line, Some(3));
    assert!(!d.capabilities.resolve && d.capabilities.reply && !d.capabilities.file_comments);
    assert_eq!(d.merge_methods, vec!["squash", "merge"]);
    assert!(d
        .reviewers
        .iter()
        .any(|r| r.login == "bob" && r.state == "requested"));
}

#[tokio::test]
async fn forgejo_submit_uses_line_positions_and_folds_file_comments() {
    let d = forgejo_detail();
    let host = FixtureHost::new(HostKind::Forgejo);
    let drafts = vec![
        draft("line", "src/p.rs", "right", Some(20), Some(18), "Name this"),
        draft("line", "src/p.rs", "left", Some(3), None, "Keep this check"),
        draft("file", "README.md", "right", None, None, "Mention the flag"),
    ];
    assert!(submit(
        &host,
        &repo(HostKind::Forgejo),
        &d,
        &Submission {
            event: "requestChanges",
            body: "",
            comments: &drafts,
            head: "h1"
        }
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("summary"));
    submit(
        &host,
        &repo(HostKind::Forgejo),
        &d,
        &Submission {
            event: "requestChanges",
            body: "Please fix",
            comments: &drafts,
            head: "h1",
        },
    )
    .await
    .unwrap();
    let writes = host.writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].1, "repos/o/r/pulls/3/reviews");
    let body = writes[0].2.clone().unwrap();
    assert_eq!(body["event"], "REQUEST_CHANGES");
    assert_eq!(body["commit_id"], "h1");
    assert_eq!(body["comments"][0]["new_position"], 20);
    assert!(body["comments"][0]["body"]
        .as_str()
        .unwrap()
        .starts_with("Lines 18–20:"));
    assert_eq!(body["comments"][1]["old_position"], 3);
    assert_eq!(body["comments"].as_array().unwrap().len(), 2);
    assert!(body["body"].as_str().unwrap().contains("**README.md**"));
}

#[tokio::test]
async fn merge_without_agent_credits_keeps_people_and_the_head_guard() {
    let mut d = forgejo_detail();
    d.commits.push(Commit {
        sha: "h2".into(),
        message: "Tests\n\nCo-authored-by: Bob <bob@example.com>".into(),
        author: "Alice".into(),
        login: None,
        date: String::new(),
        url: None,
    });
    let host = FixtureHost::new(HostKind::Forgejo);
    merge(
        &host,
        &repo(HostKind::Forgejo),
        &d,
        &MergeInput {
            method: "squash",
            expected_head: "h1",
            remove_credits: true,
        },
    )
    .await
    .unwrap();
    let body = host.writes()[0].2.clone().unwrap();
    assert_eq!(body["Do"], "squash");
    assert_eq!(body["head_commit_id"], "h1");
    assert_eq!(body["MergeTitleField"], "WIP: Parser (#3)");
    let message = body["MergeMessageField"].as_str().unwrap();
    assert!(!message.contains("codex@openai.com"));
    assert!(message.contains("Co-authored-by: Bob <bob@example.com>"));
    assert!(
        merge(
            &host,
            &repo(HostKind::Forgejo),
            &d,
            &MergeInput {
                method: "rebase",
                expected_head: "h1",
                remove_credits: false
            }
        )
        .await
        .is_err(),
        "rebase is not allowed by this repository"
    );
}

#[tokio::test]
async fn github_merge_reads_host_message_after_the_guard_and_strips_credits() {
    let mut d = github_detail();
    d.summary.state = "open".into();
    d.capabilities.merge = true;
    d.merge_methods = vec!["squash".into()];
    let head = d.summary.head_sha.clone();
    let host = FixtureHost::new(HostKind::Github).answer(
        "GQL",
        "viewerMergeBodyText",
        json!({"data":{"repository":{"pullRequest":{"headRefOid":head,"viewerMergeHeadlineText":"Revoke tokens (#15100)","viewerMergeBodyText":"* Revoke\n\nCo-authored-by: Ada <ada@example.com>\nCo-Authored-By: Claude <noreply@anthropic.com>"}}}}),
    );
    merge(
        &host,
        &repo(HostKind::Github),
        &d,
        &MergeInput {
            method: "squash",
            expected_head: &head,
            remove_credits: true,
        },
    )
    .await
    .unwrap();
    let put = host
        .requests()
        .into_iter()
        .find(|(m, _, _)| m == "PUT")
        .unwrap();
    let body = put.2.unwrap();
    assert_eq!(body["sha"], head);
    assert_eq!(body["commit_title"], "Revoke tokens (#15100)");
    assert_eq!(
        body["commit_message"],
        "* Revoke\n\nCo-authored-by: Ada <ada@example.com>"
    );
    // A push between the guard and the message read fails the merge.
    let moved = FixtureHost::new(HostKind::Github).answer(
        "GQL",
        "viewerMergeBodyText",
        json!({"data":{"repository":{"pullRequest":{"headRefOid":"newer","viewerMergeHeadlineText":"t","viewerMergeBodyText":"b"}}}}),
    );
    assert!(merge(
        &moved,
        &repo(HostKind::Github),
        &d,
        &MergeInput {
            method: "squash",
            expected_head: &head,
            remove_credits: true
        }
    )
    .await
    .is_err());
    assert!(!moved.requests().iter().any(|(m, _, _)| m == "PUT"));
}

fn gitlab_detail() -> PullDetail {
    let mr = json!({"iid":5,"title":"Draft: Cache","web_url":"https://gitlab.com/o/r/-/merge_requests/5","state":"opened","draft":true,"author":{"username":"alice"},"source_branch":"cache","target_branch":"main","sha":"g2","diff_refs":{"base_sha":"g0","start_sha":"g0","head_sha":"g2"},"detailed_merge_status":"mergeable","reviewers":[{"id":7,"username":"bob"}],"head_pipeline":{"id":99,"status":"failed"},"user":{"can_merge":true},"labels":["perf"]});
    let discussions = vec![
        json!({"id":"d1","individual_note":false,"notes":[{"id":1,"type":"DiffNote","body":"Why 64?","author":{"username":"bob"},"created_at":"2026-11-26T10:00:00Z","resolvable":true,"resolved":false,"position":{"new_path":"src/c.rs","old_path":"src/c.rs","new_line":8,"old_line":null,"position_type":"text","head_sha":"g2"}},{"id":2,"type":"DiffNote","body":"Measured","author":{"username":"alice"},"created_at":"2026-11-26T10:05:00Z","resolvable":true,"resolved":false}]}),
        json!({"id":"d2","individual_note":true,"notes":[{"id":3,"type":null,"body":"Ping","author":{"username":"bob"},"created_at":"2026-11-26T09:00:00Z"}]}),
        json!({"id":"d3","notes":[{"id":4,"system":true,"body":"added 1 commit","author":{"username":"alice"}}]}),
    ];
    let jobs = vec![
        json!({"id":11,"name":"test","stage":"test","status":"failed","web_url":"https://gitlab.com/o/r/-/jobs/11"}),
    ];
    gitlab::map_detail(
        &repo(HostKind::Gitlab),
        &mr,
        &json!({"permissions":{"project_access":{"access_level":40}}}),
        Some("carol".into()),
        &discussions,
        &json!({"approved":false,"approved_by":[]}),
        &[],
        &jobs,
    )
}

#[test]
fn gitlab_maps_discussions_pipelines_and_line_pairs() {
    let d = gitlab_detail();
    assert_eq!(d.threads.len(), 1);
    assert_eq!(d.threads[0].comments.len(), 2);
    assert_eq!(d.timeline.len(), 1, "system notes are skipped");
    assert_eq!(d.check_runs[0].rerun.as_deref(), Some("pipeline:99"));
    assert_eq!(d.summary.checks.as_deref(), Some("failed"));
    let patch = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,4 @@\n a\n-b\n+B\n+C\n c\n";
    assert_eq!(
        gitlab::line_pair(patch, "right", 1),
        Some((Some(1), Some(1)))
    );
    assert_eq!(gitlab::line_pair(patch, "right", 3), Some((None, Some(3))));
    assert_eq!(gitlab::line_pair(patch, "left", 2), Some((Some(2), None)));
    assert_eq!(
        gitlab::line_pair(patch, "right", 4),
        Some((Some(3), Some(4)))
    );
    assert_eq!(gitlab::line_pair(patch, "right", 40), None);
}

#[tokio::test]
async fn gitlab_submit_posts_positions_then_approves_with_the_reviewed_sha() {
    let d = gitlab_detail();
    let host = FixtureHost::new(HostKind::Gitlab).answer(
        "GET",
        "projects/o%2Fr/merge_requests/5/diffs",
        json!([{"old_path":"src/c.rs","new_path":"src/c.rs","diff":"@@ -6,3 +6,4 @@\n x\n y\n+z\n w\n"}]),
    );
    let drafts = vec![draft(
        "line",
        "src/c.rs",
        "right",
        Some(7),
        None,
        "Context line",
    )];
    submit(
        &host,
        &repo(HostKind::Gitlab),
        &d,
        &Submission {
            event: "approve",
            body: "Nice",
            comments: &drafts,
            head: "g2",
        },
    )
    .await
    .unwrap();
    let writes = host.writes();
    assert_eq!(writes.len(), 3);
    let position = &writes[0].2.as_ref().unwrap()["position"];
    assert_eq!(
        (position["old_line"].as_u64(), position["new_line"].as_u64()),
        (Some(7), Some(7))
    );
    assert_eq!(position["head_sha"], "g2");
    assert!(writes[1].1.ends_with("/notes"));
    assert!(writes[2].1.ends_with("/approve"));
    assert_eq!(writes[2].2.as_ref().unwrap()["sha"], "g2");
    // A line that left the diff refuses the whole review before anything posts.
    let host = FixtureHost::new(HostKind::Gitlab).answer(
        "GET",
        "projects/o%2Fr/merge_requests/5/diffs",
        json!([]),
    );
    let gone = vec![draft("line", "src/c.rs", "right", Some(70), None, "Gone")];
    assert!(submit(
        &host,
        &repo(HostKind::Gitlab),
        &d,
        &Submission {
            event: "comment",
            body: "",
            comments: &gone,
            head: "g2"
        }
    )
    .await
    .is_err());
    assert!(host.writes().is_empty());
}

#[test]
fn bitbucket_and_azure_read_threads_and_votes() {
    let pr = json!({"id":4,"title":"Docs","state":"OPEN","author":{"nickname":"dee"},"source":{"branch":{"name":"docs"},"commit":{"hash":"bb1"}},"destination":{"branch":{"name":"main"},"commit":{"hash":"bb0"}},"links":{"html":{"href":"https://bitbucket.org/o/r/pull-requests/4"}},"participants":[{"user":{"nickname":"eve"},"role":"REVIEWER","approved":true,"state":"approved"}]});
    let comments = json!([
        {"id":1,"content":{"raw":"Typo"},"user":{"nickname":"eve"},"inline":{"path":"README.md","to":3}},
        {"id":2,"content":{"raw":"Fixed"},"user":{"nickname":"dee"},"parent":{"id":1}},
        {"id":3,"content":{"raw":"Thanks"},"user":{"nickname":"eve"}}
    ]);
    let d = other::bitbucket_map(
        &repo(HostKind::Bitbucket),
        &pr,
        &comments,
        &json!([]),
        &json!([{"key":"ci","state":"FAILED"}]),
        None,
    );
    assert_eq!(d.threads.len(), 1);
    assert_eq!(d.threads[0].comments.len(), 2);
    assert_eq!(d.timeline.len(), 1);
    assert_eq!(d.summary.review_decision.as_deref(), Some("approved"));
    assert!(!d.capabilities.merge, "no atomic head guard on Bitbucket");
    let az = json!({"pullRequestId":8,"title":"Az","status":"active","createdBy":{"uniqueName":"fay@example.com"},"sourceRefName":"refs/heads/az","targetRefName":"refs/heads/main","lastMergeSourceCommit":{"commitId":"a1"},"reviewers":[{"uniqueName":"gus@example.com","vote":-5}]});
    let threads = json!([{"id":10,"status":"active","threadContext":{"filePath":"/src/x.cs","rightFileStart":{"line":4},"rightFileEnd":{"line":6}},"comments":[{"id":1,"content":"Nit","author":{"uniqueName":"gus@example.com"},"commentType":"text"}]},{"id":11,"status":"active","comments":[{"id":1,"content":"System","commentType":"system"}]}]);
    let d = other::azure_map(
        &repo(HostKind::Azure),
        &az,
        &threads,
        &json!([]),
        &json!([]),
        &json!({"id":"me"}),
    );
    assert_eq!(d.threads.len(), 1);
    assert_eq!(
        (d.threads[0].start_line, d.threads[0].line),
        (Some(4), Some(6))
    );
    assert_eq!(d.threads[0].path, "src/x.cs");
    assert_eq!(
        d.summary.review_decision.as_deref(),
        Some("changesRequested")
    );
    assert_eq!(
        d.summary.url,
        "https://dev.azure.com/org/project/_git/r/pullrequest/8"
    );
}

#[test]
fn raw_diffs_split_into_files_with_blob_revisions() {
    let diff = "diff --git a/a.txt b/a.txt\nindex 1111111..2222222 100644\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1,2 @@\n-x\n+y\n+z\ndiff --git a/old.txt b/new.txt\nsimilarity index 90%\nrename from old.txt\nrename to new.txt\ndiff --git a/bin.png b/bin.png\nnew file mode 100644\nindex 0000000..3333333\nBinary files /dev/null and b/bin.png differ\n";
    let files = files_from_diff(diff);
    assert_eq!(files.len(), 3);
    assert_eq!(
        (
            files[0].additions,
            files[0].deletions,
            files[0].digest.as_str()
        ),
        (2, 1, "2222222")
    );
    assert_eq!(
        (
            files[1].status.as_str(),
            files[1].old_path.as_deref(),
            files[1].path.as_str()
        ),
        ("renamed", Some("old.txt"), "new.txt")
    );
    assert!(files[2].binary && files[2].status == "added");
}

#[test]
fn drafts_viewed_marks_guides_and_views_persist_per_author() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::Database::open(&dir.path().join("review.sqlite")).unwrap();
    let key = "github.com/o/r#1";
    let line = store::save_draft(
        &db,
        key,
        "owner",
        &json!({"kind":"line","path":"a.rs","line":5,"startLine":3,"body":"Why?","headSha":"h1"}),
    )
    .unwrap();
    assert_eq!(
        (line.line, line.start_line, line.side.as_deref()),
        (Some(5), Some(3), Some("right"))
    );
    store::save_draft(
        &db,
        key,
        "owner",
        &json!({"kind":"summary","body":"Overall"}),
    )
    .unwrap();
    let summary = store::save_draft(
        &db,
        key,
        "owner",
        &json!({"kind":"summary","body":"Overall, good"}),
    )
    .unwrap();
    store::save_draft(
        &db,
        key,
        "thread:t1",
        &json!({"kind":"file","path":"b.rs","body":"Agent note"}),
    )
    .unwrap();
    let owner = store::drafts(&db, key, "owner").unwrap();
    assert_eq!(
        owner.len(),
        2,
        "one summary per author; the agent's draft is separate"
    );
    assert_eq!(store::drafts(&db, key, "thread:t1").unwrap().len(), 1);
    assert!(store::save_draft(
        &db,
        key,
        "owner",
        &json!({"kind":"line","path":"/etc/passwd","line":1,"body":"x"})
    )
    .is_err());
    assert!(store::save_draft(
        &db,
        key,
        "owner",
        &json!({"kind":"line","path":"a","line":2,"startLine":4,"body":"x"})
    )
    .is_err());
    // An edit made while a submit was in flight survives the clear.
    let sent = vec![line.clone(), summary.clone()];
    std::thread::sleep(std::time::Duration::from_millis(5));
    store::save_draft(
        &db,
        key,
        "owner",
        &json!({"id":line.id,"kind":"line","path":"a.rs","line":5,"body":"Why? (edited)"}),
    )
    .unwrap();
    store::clear_submitted(&db, key, "owner", &sent).unwrap();
    let left = store::drafts(&db, key, "owner").unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].body, "Why? (edited)");
    // Viewed marks: same revision → viewed, a new push → changed.
    store::set_viewed(&db, key, "a.rs", Some("rev1")).unwrap();
    store::set_viewed(&db, key, "b.rs", Some("rev1")).unwrap();
    let file = |path: &str, digest: &str| PullFile {
        path: path.into(),
        old_path: None,
        status: "modified".into(),
        additions: 0,
        deletions: 0,
        patch: None,
        binary: false,
        digest: digest.into(),
        viewed: "unviewed".into(),
    };
    let mut files = vec![
        file("a.rs", "rev1"),
        file("b.rs", "rev2"),
        file("c.rs", "rev1"),
    ];
    store::apply_viewed(&db, key, &mut files).unwrap();
    assert_eq!(
        files.iter().map(|f| f.viewed.as_str()).collect::<Vec<_>>(),
        ["viewed", "changed", "unviewed"]
    );
    store::set_viewed(&db, key, "a.rs", None).unwrap();
    store::apply_viewed(&db, key, &mut files[..1]).unwrap();
    store::save_guide(&db, key, "h1", &json!({"headSha":"h1","stops":[]})).unwrap();
    assert_eq!(store::guide(&db, key).unwrap().unwrap()["headSha"], "h1");
    let views = store::save_views(
        &db,
        &json!([{"id":"v1","name":"Needs me","query":{"involvement":"reviewing"}}]),
    )
    .unwrap();
    assert_eq!(store::views(&db).unwrap(), views);
    assert!(store::save_views(&db, &json!([{"id":"v1","name":"","query":{}}])).is_err());
}

#[test]
fn guide_output_is_clamped_to_the_real_diff() {
    let diff = "diff --git a/src/api.rs b/src/api.rs\nindex 1..2 100644\n--- a/src/api.rs\n+++ b/src/api.rs\n@@ -10,4 +10,6 @@ fn x()\n a\n-b\n+B\n+C\n+D\n c\n d\ndiff --git a/src/ui.rs b/src/ui.rs\nindex 3..4 100644\n--- a/src/ui.rs\n+++ b/src/ui.rs\n@@ -1,2 +1,3 @@\n u\n+v\n w\ndiff --git a/Cargo.lock b/Cargo.lock\nindex 5..6 100644\n--- a/Cargo.lock\n+++ b/Cargo.lock\n@@ -1 +1 @@\n-l\n+L\n";
    let raw = guide::parse_json("Here is the guide:\n```json\n{\"stops\":[{\"title\":\"The API gains a field\",\"summary\":\"s\",\"attention\":{\"kind\":\"contract\",\"note\":\"Wire format changes\"},\"excerpts\":[{\"path\":\"src/api.rs\",\"side\":\"new\",\"start\":5,\"end\":12,\"caption\":\"new field\"},{\"path\":\"missing.rs\",\"side\":\"new\",\"start\":1,\"end\":2}],\"notes\":[{\"path\":\"src/api.rs\",\"line\":11,\"title\":\"Why\",\"body\":\"b\"},{\"path\":\"src/api.rs\",\"line\":99,\"title\":\"Outside\",\"body\":\"b\"}],\"files\":[{\"path\":\"src/api.rs\",\"note\":\"dup\"}],\"questions\":[\"Why now?\",\"a\",\"b\"]},{\"title\":\"UI\",\"summary\":\"\",\"attention\":{\"kind\":\"bug\",\"note\":\"x\"},\"excerpts\":[],\"files\":[{\"path\":\"src/ui.rs\",\"note\":\"wired\"},{\"path\":\"src/ui.rs\",\"note\":\"twice\"}]}]}\n```").unwrap();
    let stops = guide::normalize(&raw, diff).unwrap();
    assert_eq!(stops.len(), 3, "an added stop collects Cargo.lock");
    let excerpt = &stops[0]["excerpts"][0];
    assert_eq!(
        (excerpt["start"].as_u64(), excerpt["end"].as_u64()),
        (Some(10), Some(12))
    );
    let patch = excerpt["patch"].as_str().unwrap();
    assert!(
        patch.contains("@@ -10,2 +10,3 @@\n a\n-b\n+B\n+C\n"),
        "{patch}"
    );
    assert_eq!(stops[0]["excerpts"].as_array().unwrap().len(), 1);
    assert_eq!(stops[0]["notes"].as_array().unwrap().len(), 1);
    assert_eq!(
        stops[0]["files"].as_array().unwrap().len(),
        0,
        "an excerpted file needs no files entry"
    );
    assert_eq!(stops[0]["questions"].as_array().unwrap().len(), 2);
    assert_eq!(stops[0]["attention"]["kind"], "contract");
    assert!(stops[1]["attention"].is_null(), "unknown kinds are dropped");
    assert_eq!(stops[1]["files"].as_array().unwrap().len(), 1);
    assert_eq!(stops[2]["title"], "Other files");
    assert_eq!(stops[2]["files"][0]["path"], "Cargo.lock");
    assert!(guide::parse_json("no json here").is_err());
}
