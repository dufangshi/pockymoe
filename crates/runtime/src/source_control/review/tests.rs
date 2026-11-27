//! Review behaviour against recorded host responses (GitHub recordings are
//! read-only captures of a public PR with logins and e-mail replaced) and hand
//! shaped GitLab/Forgejo responses from their API references.
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
            HostKind::Forgejo => "127.0.0.1:19173",
        }
        .into(),
        owner: "o".into(),
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

#[tokio::test]
async fn github_rerun_and_revert_use_the_run_and_the_pr_node() {
    let mut d = github_detail();
    d.capabilities.rerun = true;
    d.capabilities.revert = true;
    d.check_runs[0].rerun = Some("run:42".into());
    let host = FixtureHost::new(HostKind::Github).answer(
        "GQL",
        "revertPullRequest",
        json!({"data":{"revertPullRequest":{"revertPullRequest":{"number":9,"url":"https://github.com/o/r/pull/9"}}}}),
    );
    rerun(&host, &repo(HostKind::Github), &d, "run:42")
        .await
        .unwrap();
    assert!(
        rerun(&host, &repo(HostKind::Github), &d, "run:7")
            .await
            .is_err(),
        "only a token the PR offered"
    );
    let opened = revert(&host, &repo(HostKind::Github), &d).await.unwrap();
    assert_eq!(opened["number"], 9);
    let requests = host.requests();
    assert_eq!(requests[0].1, "repos/o/r/actions/runs/42/rerun-failed-jobs");
    assert_eq!(
        requests[1].2.as_ref().unwrap()["variables"]["id"],
        d.node_id.clone().unwrap()
    );
    // Open PRs cannot be reverted.
    d.summary.state = "open".into();
    assert!(revert(&host, &repo(HostKind::Github), &d).await.is_err());
}

fn summary_at(number: u64, head: &str, base: &str) -> PullSummary {
    PullSummary {
        stack: None,
        url: format!("https://127.0.0.1:1/o/r/pulls/{number}"),
        number,
        title: format!("Layer {number}"),
        state: "open".into(),
        draft: false,
        author: None,
        repository: repo(HostKind::Forgejo),
        head_ref: head.into(),
        base_ref: base.into(),
        head_sha: format!("sha-{number}"),
        created_at: String::new(),
        updated_at: String::new(),
        checks: None,
        review_decision: None,
        requested_reviewers: Vec::new(),
        additions: None,
        deletions: None,
        changed_files: None,
        comments: None,
        labels: Vec::new(),
    }
}

#[test]
fn stacks_are_inferred_from_bases_and_github_stacks_are_read() {
    let mut pulls = vec![
        summary_at(3, "c", "b"),
        summary_at(1, "a", "main"),
        summary_at(2, "b", "a"),
        summary_at(9, "x", "main"),
        summary_at(4, "d", "c"),
    ];
    pulls[4].state = "merged".into();
    stack::infer(&mut pulls);
    let at = |n: u64| pulls.iter().find(|p| p.number == n).unwrap().stack.clone();
    assert_eq!(at(1).map(|s| (s.position, s.size)), Some((1, 3)));
    assert_eq!(at(2).map(|s| (s.position, s.size)), Some((2, 3)));
    assert_eq!(at(3).map(|s| s.position), Some(3));
    assert_eq!(at(9), None, "a lone PR is not a stack");
    assert_eq!(at(4), None, "merged PRs are not layers");
    let found = stack::inferred(&pulls, 2).unwrap();
    assert_eq!(found.base, "main");
    assert_eq!(
        found.layers.iter().map(|l| l.number).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert!(!found.native);
    // Lower layers merged: the stack's base is found through them, and they stay as merged layers.
    let mut rest = vec![summary_at(3, "c", "b"), summary_at(4, "d", "c")];
    rest[0].created_at = "2026-11-26T10:00:00Z".into();
    let mut done = vec![
        summary_at(2, "b", "a"),
        summary_at(1, "a", "main"),
        summary_at(8, "a", "old"),
    ];
    for p in &mut done {
        p.state = "merged".into();
    }
    done[0].created_at = "2026-11-26T09:00:00Z".into();
    done[0].updated_at = "2026-11-26T11:00:00Z".into();
    done[1].created_at = "2026-11-26T08:00:00Z".into();
    done[1].updated_at = "2026-11-26T11:30:00Z".into();
    done[2].updated_at = "2025-01-01T00:00:00Z".into(); // an old PR from a reused branch name
    let mut settled = stack::inferred(&rest, 4).unwrap();
    assert_eq!(settled.base, "b");
    stack::settle_base(&mut settled, &rest, &done);
    assert_eq!(settled.base, "main");
    assert_eq!(
        settled
            .layers
            .iter()
            .map(|l| (l.number, l.state.as_str()))
            .collect::<Vec<_>>(),
        [(1, "merged"), (2, "merged"), (3, "open"), (4, "open")]
    );
    // Two open PRs from the same branch name (forks) give no parent.
    let mut forks = vec![
        summary_at(1, "a", "main"),
        summary_at(5, "a", "main"),
        summary_at(2, "b", "a"),
    ];
    stack::infer(&mut forks);
    assert!(forks.iter().all(|p| p.stack.is_none()));
    // A recorded GitHub stack (cli/cli, logins replaced): bottom to top, merged layers kept.
    let recorded: Value = serde_json::from_str(include_str!("fixtures/github-stack.json")).unwrap();
    let native = stack::map_native(&repo(HostKind::Github), &recorded).unwrap();
    assert!(native.native);
    assert_eq!(native.base, "trunk");
    assert_eq!(
        native
            .layers
            .iter()
            .map(|l| (l.number, l.state.as_str()))
            .collect::<Vec<_>>(),
        [(15100, "merged"), (15152, "merged")]
    );
    assert_eq!(native.layers[1].base_ref, native.layers[0].head_ref);
}

#[test]
fn stack_merges_cover_the_layers_below_and_guard_every_head() {
    let mut pulls = vec![
        summary_at(1, "a", "main"),
        summary_at(2, "b", "a"),
        summary_at(3, "c", "b"),
    ];
    let stack = stack::inferred(&pulls, 1).unwrap();
    let range = stack::merge_range(&stack, 2).unwrap();
    assert_eq!(range.iter().map(|l| l.number).collect::<Vec<_>>(), [1, 2]);
    let heads = |pairs: &[(u64, &str)]| {
        pairs
            .iter()
            .map(|(n, h)| stack::StackHead {
                number: *n,
                head_sha: (*h).into(),
            })
            .collect::<Vec<_>>()
    };
    assert!(stack::guard_heads(&range, &heads(&[(1, "sha-1"), (2, "sha-2")])).is_ok());
    assert!(stack::guard_heads(&range, &heads(&[(2, "sha-2")]))
        .unwrap_err()
        .to_string()
        .contains("changed"));
    assert!(
        stack::guard_heads(&range, &heads(&[(1, "old"), (2, "sha-2")]))
            .unwrap_err()
            .to_string()
            .contains("moved")
    );
    pulls[0].draft = true;
    let drafted = stack::inferred(&pulls, 1).unwrap();
    assert!(stack::merge_range(&drafted, 3)
        .unwrap_err()
        .to_string()
        .contains("draft"));
}

#[tokio::test]
async fn github_stack_merge_is_one_merge_async_call_with_the_target_head() {
    let repo = repo(HostKind::Github);
    let stack = stack::Stack {
        id: "7".into(),
        native: true,
        base: "main".into(),
        layers: vec![
            stack::StackLayer {
                number: 1,
                url: String::new(),
                title: String::new(),
                state: "open".into(),
                draft: false,
                head_ref: "a".into(),
                head_sha: "h1".into(),
                base_ref: "main".into(),
            },
            stack::StackLayer {
                number: 2,
                url: String::new(),
                title: String::new(),
                state: "open".into(),
                draft: false,
                head_ref: "b".into(),
                head_sha: "h2".into(),
                base_ref: "a".into(),
            },
        ],
    };
    let host = FixtureHost::new(HostKind::Github)
        .answer("GET", "repos/o/r/pulls/1", json!({"head":{"sha":"h1"}}))
        .answer(
            "GET",
            "repos/o/r/pulls/2/merge-async/u1",
            json!({"status":"merged","details":{"sha":"m"}}),
        )
        .answer("GET", "repos/o/r/pulls/2", json!({"head":{"sha":"h2"}}))
        .answer(
            "PUT",
            "repos/o/r/pulls/2/merge-async",
            json!({"status":"pending","details":{"uuid":"u1"}}),
        );
    let expected = vec![
        stack::StackHead {
            number: 1,
            head_sha: "h1".into(),
        },
        stack::StackHead {
            number: 2,
            head_sha: "h2".into(),
        },
    ];
    let result = stack::merge_stack(
        &host,
        &repo,
        &stack::MergeStack {
            stack: &stack,
            target: 2,
            method: "squash",
            expected: &expected,
            local: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(result["merged"], json!([1, 2]));
    assert_eq!(result["result"]["status"], "merged");
    let put = host
        .requests()
        .into_iter()
        .find(|(m, _, _)| m == "PUT")
        .unwrap();
    assert_eq!(
        put.2.unwrap(),
        json!({"merge_method":"squash","merge_action":"default","sha":"h2"})
    );
    // A layer that moved refuses before any write.
    let moved = FixtureHost::new(HostKind::Github)
        .answer("GET", "repos/o/r/pulls/1", json!({"head":{"sha":"newer"}}))
        .answer("GET", "repos/o/r/pulls/2", json!({"head":{"sha":"h2"}}));
    assert!(stack::merge_stack(
        &moved,
        &repo,
        &stack::MergeStack {
            stack: &stack,
            target: 2,
            method: "squash",
            expected: &expected,
            local: None
        }
    )
    .await
    .is_err());
    assert!(moved.requests().iter().all(|(m, _, _)| m == "GET"));
    // Squash-merging an inferred stack without a local checkout is refused up front.
    let inferred = stack::Stack {
        native: false,
        ..stack.clone()
    };
    let host = FixtureHost::new(HostKind::Forgejo)
        .answer("GET", "repos/o/r/pulls/1", json!({"head":{"sha":"h1"}}))
        .answer("GET", "repos/o/r/pulls/2", json!({"head":{"sha":"h2"}}));
    let error = stack::merge_stack(
        &host,
        &repo_kind(HostKind::Forgejo),
        &stack::MergeStack {
            stack: &inferred,
            target: 2,
            method: "squash",
            expected: &expected,
            local: None,
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("workspace"));
}

fn repo_kind(kind: HostKind) -> Repository {
    repo(kind)
}

/// The cascade on real Git: a bare "remote", a local clone that is never
/// touched, three stacked branches, the bottom squash-merged into main.
#[test]
fn rebase_stack_replays_layers_in_a_temporary_clone_and_leases_each_push() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let run = |cwd: &std::path::Path, args: &[&str]| {
        let out = Command::new("git")
            .current_dir(cwd)
            .args([
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    let remote = dir.path().join("remote.git");
    let local = dir.path().join("local");
    run(
        dir.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    run(
        dir.path(),
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            local.to_str().unwrap(),
        ],
    );
    let write = |name: &str, text: &str| std::fs::write(local.join(name), text).unwrap();
    write("base.txt", "base\n");
    run(&local, &["add", "."]);
    run(&local, &["commit", "-qm", "base"]);
    run(&local, &["push", "-q", "origin", "HEAD:main"]);
    let mut heads = Vec::new();
    for (branch, from) in [("a", "main"), ("b", "a"), ("c", "b")] {
        run(&local, &["checkout", "-q", "-B", branch, from]);
        write(&format!("{branch}.txt"), branch);
        run(&local, &["add", "."]);
        run(&local, &["commit", "-qm", branch]);
        run(&local, &["push", "-q", "origin", branch]);
        heads.push(run(&local, &["rev-parse", "HEAD"]));
    }
    // Squash-merge layer a into main on the remote side.
    run(&local, &["checkout", "-q", "main"]);
    run(&local, &["merge", "-q", "--squash", "a"]);
    run(&local, &["commit", "-qm", "a (squashed)"]);
    run(&local, &["push", "-q", "origin", "main"]);
    run(&local, &["checkout", "-q", "c"]);
    let before = run(&local, &["rev-parse", "HEAD"]);
    let layers: Vec<stack::StackLayer> = [("b", 1usize), ("c", 2)]
        .iter()
        .enumerate()
        .map(|(i, (branch, at))| stack::StackLayer {
            number: i as u64 + 2,
            url: String::new(),
            title: String::new(),
            state: "open".into(),
            draft: false,
            head_ref: (*branch).into(),
            head_sha: heads[*at].clone(),
            base_ref: if i == 0 { "a".into() } else { "b".into() },
        })
        .collect();
    let url = format!("file://{}", remote.display());
    // A stale lease refuses.
    let mut stale = layers.clone();
    stale[0].head_sha = heads[0].clone();
    assert!(stack::replay(&local, &url, "main", &stale, None)
        .unwrap_err()
        .to_string()
        .contains("moved"));
    let rebased = stack::replay(&local, &url, "main", &layers, Some(&heads[0])).unwrap();
    assert_eq!(rebased.len(), 2);
    let main = run(&local, &["ls-remote", &url, "refs/heads/main"]);
    let b = run(&local, &["ls-remote", &url, "refs/heads/b"]);
    assert_eq!(b.split_whitespace().next(), Some(rebased[0].1.as_str()));
    // b now sits directly on the squashed main and holds only its own commit.
    let log = run(
        &remote,
        &[
            "log",
            "--format=%s",
            &format!(
                "{}..{}",
                main.split_whitespace().next().unwrap(),
                rebased[1].1
            ),
        ],
    );
    assert_eq!(log.lines().collect::<Vec<_>>(), ["c", "b"]);
    // The user's checkout did not move.
    assert_eq!(run(&local, &["rev-parse", "HEAD"]), before);
    assert_eq!(run(&local, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn a_stacked_github_pr_joins_the_stack_below_or_starts_one() {
    let repo = repo(HostKind::Github);
    let host = FixtureHost::new(HostKind::Github).answer(
        "GET",
        "repos/o/r/stacks?pull_request=5",
        json!([{"number":9}]),
    );
    stack::join_native(&host, &repo, 5, 6).await.unwrap();
    let posted = host
        .requests()
        .into_iter()
        .find(|(m, _, _)| m == "POST")
        .unwrap();
    assert_eq!(
        (posted.1.as_str(), posted.2.unwrap()),
        ("repos/o/r/stacks/9/add", json!({"pull_requests":[6]}))
    );
    let host = FixtureHost::new(HostKind::Github).answer(
        "GET",
        "repos/o/r/stacks?pull_request=5",
        json!([]),
    );
    stack::join_native(&host, &repo, 5, 6).await.unwrap();
    let posted = host
        .requests()
        .into_iter()
        .find(|(m, _, _)| m == "POST")
        .unwrap();
    assert_eq!(
        (posted.1.as_str(), posted.2.unwrap()),
        ("repos/o/r/stacks", json!({"pull_requests":[5, 6]}))
    );
    assert!(
        stack::join_native(&host, &repo_kind(HostKind::Gitlab), 5, 6)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn forgejo_list_reads_the_review_decision_of_open_pulls() {
    let pull = |n: u64, state: &str| json!({"number":n,"title":"P","html_url":"u","state":state,"merged":false,"user":{"login":"alice"},"head":{"ref":"f","sha":format!("h{n}")},"base":{"ref":"main","sha":"b"},"requested_reviewers":[]});
    let host = FixtureHost::new(HostKind::Forgejo)
        .answer("GET", "repos/o/r/pulls?", json!([pull(1, "open"), pull(2, "open"), pull(3, "closed")]))
        .answer("GET", "repos/o/r/commits/", json!({"state":"success","statuses":[]}))
        // PR 1: approved, then a plain comment by the same reviewer.
        .answer("GET", "repos/o/r/pulls/1/reviews", json!([
            {"id":1,"user":{"login":"bob"},"state":"APPROVED","body":""},
            {"id":2,"user":{"login":"bob"},"state":"COMMENT","body":"nit"},
        ]))
        // PR 2: changes requested, later dismissed.
        .answer("GET", "repos/o/r/pulls/2/reviews", json!([
            {"id":3,"user":{"login":"carol"},"state":"REQUEST_CHANGES","body":"no","dismissed":true},
        ]));
    let pulls = forgejo::list(&host, &repo(HostKind::Forgejo), "all")
        .await
        .unwrap();
    let decision = |n: u64| {
        pulls
            .iter()
            .find(|p| p.number == n)
            .unwrap()
            .review_decision
            .clone()
    };
    assert_eq!(decision(1).as_deref(), Some("approved"));
    assert_eq!(decision(2), None);
    assert_eq!(decision(3), None);
    assert!(
        !host
            .requests()
            .iter()
            .any(|(_, p, _)| p == "repos/o/r/pulls/3/reviews"),
        "closed PRs are not read"
    );
}
