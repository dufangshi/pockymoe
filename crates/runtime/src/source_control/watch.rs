use super::*;
use std::sync::Arc;
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchSnapshot {
    pub state: String,
    pub checks: String,
    pub comments: Vec<String>,
    pub conflicts: bool,
    pub queued: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchState {
    pub active: bool,
    pub failures: u8,
    pub comment_wakes: u8,
    pub stopped_reason: Option<String>,
    pub previous: Option<WatchSnapshot>,
}
impl WatchState {
    pub fn read(&mut self, snapshot: Result<WatchSnapshot>) -> Vec<&'static str> {
        if !self.active {
            return vec![];
        }
        let Ok(next) = snapshot else {
            self.failures = self.failures.saturating_add(1);
            if self.failures >= 8 {
                self.active = false;
                self.stopped_reason = Some("Eight consecutive read failures".into());
            }
            return vec![];
        };
        self.failures = 0;
        let mut reasons = Vec::new();
        let old = self.previous.as_ref();
        if next.state == "merged" || next.state == "closed" {
            reasons.push(if next.state == "merged" {
                "PR merged"
            } else {
                "PR closed"
            });
            self.active = false;
            self.stopped_reason = Some(next.state.clone());
        }
        if next.checks == "failed" && old.is_none_or(|o| o.checks != "failed") {
            reasons.push("Checks failed");
        }
        if next.checks == "passed" && old.is_some_and(|o| o.checks != "passed") {
            reasons.push("Required checks passed");
        }
        if next.conflicts && old.is_none_or(|o| !o.conflicts) {
            reasons.push("Merge conflicts");
        }
        if old.is_some_and(|o| o.queued && !next.queued) && next.state == "open" {
            reasons.push("Removed from merge queue");
        }
        if old.is_some_and(|o| next.comments.iter().any(|id| !o.comments.contains(id))) {
            reasons.push("New comments or reviews");
        }
        if reasons == ["New comments or reviews"] {
            self.comment_wakes = self.comment_wakes.saturating_add(1);
            if self.comment_wakes >= 10 {
                self.active = false;
                self.stopped_reason = Some("Ten consecutive comment-only wakes".into());
            }
        } else if !reasons.is_empty() {
            self.comment_wakes = 0;
        }
        self.previous = Some(next);
        reasons
    }
}
#[derive(Serialize, Deserialize)]
struct SavedWatch {
    repository: Repository,
    number: u64,
    state: WatchState,
}
impl Supervisor {
    pub fn source_watch_status(&self, thread: &str) -> Result<Value> {
        self.get_thread(thread)?;
        Ok(self
            .db
            .get_kv(&format!("pr-watch:{thread}"))?
            .map(|v| serde_json::from_str(&v))
            .transpose()?
            .unwrap_or(json!({"state":{"active":false}})))
    }
    pub fn source_unwatch(&self, thread: &str) -> Result<Value> {
        let mut value = self.source_watch_status(thread)?;
        value["state"]["active"] = json!(false);
        value["state"]["stoppedReason"] = json!("Stopped by user");
        self.db
            .set_kv(&format!("pr-watch:{thread}"), &value.to_string())?;
        self.source_control_changed(thread, "pullRequestWatchChanged")?;
        Ok(value)
    }
    pub async fn source_watch(
        &self,
        thread: &str,
        repo: &Repository,
        number: u64,
    ) -> Result<Value> {
        ensure!(
            self.get_thread(thread)?.parent_thread_id.is_none(),
            "Sub-agents cannot watch PRs; ask the parent thread"
        );
        ensure!(
            matches!(
                repo.kind,
                HostKind::Github | HostKind::Gitlab | HostKind::Forgejo
            ),
            "PR watch currently supports GitHub, GitLab and Forgejo"
        );
        let snapshot = self.read_pr_snapshot(repo, number).await?;
        ensure!(snapshot.state == "open", "PR is already closed or merged");
        let state = WatchState {
            active: true,
            previous: Some(snapshot),
            ..Default::default()
        };
        let value = serde_json::to_value(SavedWatch {
            repository: repo.clone(),
            number,
            state,
        })?;
        self.source_link(thread, &repo.pull_request_url(number), false)?;
        self.db
            .set_kv(&format!("pr-watch:{thread}"), &value.to_string())?;
        self.source_control_changed(thread, "pullRequestWatchChanged")?;
        Ok(value)
    }
    pub(crate) async fn read_pr_snapshot(
        &self,
        repo: &Repository,
        number: u64,
    ) -> Result<WatchSnapshot> {
        let host = self.source_host(repo)?;
        if repo.kind == HostKind::Github {
            let value=host.request("POST","graphql",Some(json!({"query":GITHUB_WATCH,"variables":{"owner":repo.owner,"name":repo.name,"number":number}}))).await;
            let snapshot = value.and_then(|value| github_snapshot(&value));
            if repo.host == "github.com" || snapshot.is_ok() {
                return snapshot;
            }
            // Older Enterprise versions may not expose merge queue fields.
        }
        let op = host.operation("show", false, Some(number), &json!({}))?;
        let detail = host.request(&op.method, &op.path, None).await?;
        let mut snapshot = WatchSnapshot {
            state: if detail["merged"].as_bool() == Some(true) || detail["state"] == "merged" {
                "merged"
            } else if detail["state"] == "closed" {
                "closed"
            } else {
                "open"
            }
            .into(),
            conflicts: detail["mergeable"].as_bool() == Some(false)
                || detail["has_conflicts"].as_bool() == Some(true),
            queued: detail["merge_queue_entry"].is_object(),
            ..Default::default()
        };
        let sha = hosts::head_oid(repo.kind, &detail);
        let op = host.operation("checks", false, Some(number), &json!({"headSha":sha}))?;
        let checks = host.request(&op.method, &op.path, None).await?;
        let checks = checks["check_runs"]
            .as_array()
            .or(checks["statuses"].as_array())
            .or_else(|| checks.as_array())
            .cloned()
            .unwrap_or_default();
        snapshot.checks = if checks.iter().any(|c| {
            matches!(
                c["conclusion"]
                    .as_str()
                    .or(c["status"].as_str())
                    .or(c["state"].as_str()),
                Some("failure" | "failed" | "cancelled" | "timed_out")
            )
        }) {
            "failed"
        } else if !checks.is_empty()
            && checks.iter().all(|c| {
                matches!(
                    c["conclusion"]
                        .as_str()
                        .or(c["status"].as_str())
                        .or(c["state"].as_str()),
                    Some("success" | "skipped" | "neutral")
                )
            })
        {
            "passed"
        } else {
            "pending"
        }
        .into();
        let identity = host.request("GET", &host.identity_path(), None).await?;
        let author = identity["login"]
            .as_str()
            .or(identity["username"].as_str())
            .unwrap_or("");
        for action in ["comments", "reviews"] {
            let op = host.operation(action, false, Some(number), &json!({}))?;
            let value = host.request(&op.method, &op.path, None).await?;
            if let Some(comments) = value.as_array() {
                for c in comments {
                    let user = c["user"]["login"]
                        .as_str()
                        .or(c["author"]["username"].as_str())
                        .unwrap_or("");
                    if user != author {
                        snapshot
                            .comments
                            .push(format!("{action}:{}:{}", c["id"], c["updated_at"]));
                    }
                }
            }
        }
        Ok(snapshot)
    }
    /// One durable poll, also used by the local host integration test.
    pub async fn poll_source_watch(&self, thread: &str) -> Result<Value> {
        let key = format!("pr-watch:{thread}");
        let Some(raw) = self.db.get_kv(&key)? else {
            return Ok(json!({"reasons":[]}));
        };
        let mut watch: SavedWatch = serde_json::from_str(&raw)?;
        if !watch.state.active {
            return Ok(json!({"reasons":[],"state":watch.state}));
        }
        let reasons = if self
            .get_thread(thread)
            .map_or(true, |t| t.closed_at.is_some())
        {
            watch.state.active = false;
            watch.state.stopped_reason = Some("Thread closed or deleted".into());
            vec![]
        } else {
            watch
                .state
                .read(self.read_pr_snapshot(&watch.repository, watch.number).await)
        };
        // User stop/replacement wins over a network read in flight.
        if self.db.get_kv(&key)?.as_deref() != Some(raw.as_str()) {
            return Ok(json!({"reasons":[]}));
        }
        if !reasons.is_empty() {
            use sha2::{Digest, Sha256};
            let request = hex::encode(Sha256::digest(serde_json::to_vec(&watch)?));
            let url = watch.repository.pull_request_url(watch.number);
            let quoted = shell_words::quote(&url);
            let prompt=format!("PR watch: {}. Inspect PR #{} in {}/{} using pockymoe pr show {} --item-url {} and pockymoe pr checks {} --item-url {} before acting.",reasons.join("; "),watch.number,watch.repository.owner,watch.repository.name,watch.number,quoted,watch.number,quoted);
            // Stable request identity makes a crash after queueing but before
            // persisting the snapshot safe to retry after restart.
            let input = serde_json::from_value(
                json!({"text":prompt,"delivery":"queue","kind":"task","subject":"PR watch","clientRequestId":format!("pr-watch:{request}")}),
            )?;
            self.send_to_thread_locally(thread, input)?;
        }
        self.db.set_kv(&key, &serde_json::to_string(&watch)?)?;
        if !reasons.is_empty() || !watch.state.active {
            self.source_control_changed(thread, "pullRequestWatchChanged")?;
        }
        Ok(json!({"reasons":reasons,"state":watch.state}))
    }
    pub fn start_source_control_worker(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(120));
            loop {
                interval.tick().await;
                let Some(s) = weak.upgrade() else { break };
                let keys: Vec<String> =
                    s.db.with(|c| {
                        let mut q = c.prepare("SELECT key FROM kv WHERE key LIKE 'pr-watch:%'")?;
                        let rows = q
                            .query_map([], |r| r.get(0))?
                            .collect::<std::result::Result<Vec<_>, _>>()?;
                        Ok(rows)
                    })
                    .unwrap_or_default();
                for key in keys {
                    let _ = s
                        .poll_source_watch(key.trim_start_matches("pr-watch:"))
                        .await;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snap() -> WatchSnapshot {
        WatchSnapshot {
            state: "open".into(),
            checks: "pending".into(),
            ..Default::default()
        }
    }
    #[test]
    fn wake_and_stop_rules_survive_serialization() {
        let mut state = WatchState {
            active: true,
            previous: Some(snap()),
            ..Default::default()
        };
        let mut next = snap();
        next.checks = "failed".into();
        next.conflicts = true;
        assert_eq!(
            state.read(Ok(next.clone())),
            vec!["Checks failed", "Merge conflicts"]
        );
        assert!(state.read(Ok(next.clone())).is_empty());
        next.checks = "passed".into();
        assert_eq!(state.read(Ok(next.clone())), vec!["Required checks passed"]);
        next.queued = true;
        state.read(Ok(next.clone()));
        next.queued = false;
        assert_eq!(
            state.read(Ok(next.clone())),
            vec!["Removed from merge queue"]
        );
        for n in 0..10 {
            next.comments.push(n.to_string());
            assert_eq!(
                state.read(Ok(next.clone())),
                vec!["New comments or reviews"]
            );
            state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        }
        assert!(!state.active);
        assert_eq!(state.comment_wakes, 10);
        let mut state = WatchState {
            active: true,
            ..Default::default()
        };
        for _ in 0..8 {
            state.read(Err(anyhow::anyhow!("offline")));
        }
        assert!(!state.active);
        for end in ["merged", "closed"] {
            let mut state = WatchState {
                active: true,
                ..Default::default()
            };
            let mut next = snap();
            next.state = end.into();
            assert_eq!(state.read(Ok(next)).len(), 1);
            assert!(!state.active);
        }
    }
}

const GITHUB_WATCH: &str = r#"query($owner:String!,$name:String!,$number:Int!){viewer{login} repository(owner:$owner,name:$name){pullRequest(number:$number){state merged mergeable mergeQueueEntry{id} comments(last:100){nodes{id updatedAt author{login}}} reviews(last:100){nodes{id updatedAt author{login}}} reviewThreads(last:100){nodes{comments(last:100){nodes{id updatedAt author{login}}}}} commits(last:1){nodes{commit{statusCheckRollup{contexts(first:100){nodes{... on CheckRun{name status conclusion isRequired(pullRequestNumber:$number)} ... on StatusContext{context state isRequired(pullRequestNumber:$number)}}}}}}}}}}"#;
fn github_snapshot(value: &Value) -> Result<WatchSnapshot> {
    let pr = &value["data"]["repository"]["pullRequest"];
    ensure!(pr.is_object(), "GitHub watch data unavailable");
    let viewer = value["data"]["viewer"]["login"].as_str().unwrap_or("");
    let contexts = pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let required = contexts
        .iter()
        .filter(|c| c["isRequired"] == true)
        .collect::<Vec<_>>();
    let failed = contexts.iter().any(|c| {
        matches!(
            c["conclusion"].as_str().or(c["state"].as_str()),
            Some("FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED")
        )
    });
    let checks = if failed {
        "failed"
    } else if !required.is_empty()
        && required.iter().all(|c| {
            matches!(
                c["conclusion"].as_str().or(c["state"].as_str()),
                Some("SUCCESS" | "NEUTRAL" | "SKIPPED")
            )
        })
    {
        "passed"
    } else {
        "pending"
    };
    let mut comments = Vec::new();
    let mut collect = |nodes: &Value| {
        for c in nodes.as_array().into_iter().flatten() {
            if c["author"]["login"]
                .as_str()
                .is_some_and(|login| login != viewer)
            {
                comments.push(format!("{}:{}", c["id"], c["updatedAt"]));
            }
        }
    };
    collect(&pr["comments"]["nodes"]);
    collect(&pr["reviews"]["nodes"]);
    for thread in pr["reviewThreads"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
    {
        collect(&thread["comments"]["nodes"]);
    }
    Ok(WatchSnapshot {
        state: if pr["merged"] == true {
            "merged"
        } else if pr["state"] == "CLOSED" {
            "closed"
        } else {
            "open"
        }
        .into(),
        checks: checks.into(),
        comments,
        conflicts: pr["mergeable"] == "CONFLICTING",
        queued: pr["mergeQueueEntry"].is_object(),
    })
}
#[cfg(test)]
mod github_tests {
    use super::*;
    #[test]
    fn required_checks_own_comments_and_merge_queue_are_normalized() {
        let mut pr = json!({"state":"OPEN","mergeable":"CONFLICTING","mergeQueueEntry":{"id":"q"},"comments":{"nodes":[{"id":"1","author":{"login":"me"}},{"id":"2","author":{"login":"other"}}]},"commits":{"nodes":[{"commit":{"statusCheckRollup":{"contexts":{"nodes":[{"isRequired":true,"conclusion":"SUCCESS"},{"isRequired":false,"status":"IN_PROGRESS"}]}}}}]}});
        let mut value = json!({"data":{"viewer":{"login":"me"},"repository":{"pullRequest":pr}}});
        let snapshot = github_snapshot(&value).unwrap();
        assert_eq!(snapshot.checks, "passed");
        assert_eq!(snapshot.comments.len(), 1);
        assert!(snapshot.conflicts);
        assert!(snapshot.queued);
        pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"][0]
            ["conclusion"] = json!("FAILURE");
        value["data"]["repository"]["pullRequest"] = pr;
        assert_eq!(github_snapshot(&value).unwrap().checks, "failed");
        assert!(github_snapshot(&json!({"errors":[]})).is_err());
    }
}
