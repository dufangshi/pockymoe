//! PR stacks: a PR whose base branch is another open PR's head sits on top of
//! it. GitHub keeps stacks itself (REST `stacks`, GraphQL `stack`/`stackEntry`,
//! `merge-async` merges a layer with every open layer below it); on other hosts,
//! and for GitHub PRs outside a host stack, the stack is inferred from bases.
//!
//! Merge stack: GitHub stacks merge in one host call; inferred stacks merge
//! bottom to top, each layer retargeted to the stack's base first and merged
//! with its head guard. After a squash or rebase merge the layers above are
//! restacked (below) so the next layer holds only its own commits.
//!
//! Rebase stack: every open layer is replayed onto the new base, bottom to
//! top, in a temporary shared clone (never the user's checkout), and each
//! branch is pushed with `--force-with-lease` on the head the reviewer saw.
use super::service::{git, valid_ref_name, KnownRepository};
use super::*;
use crate::source_control::hosts::Host;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StackLayer {
    pub number: u64,
    pub url: String,
    pub title: String,
    /// `open`, `closed` or `merged`.
    pub state: String,
    pub draft: bool,
    pub head_ref: String,
    pub head_sha: String,
    pub base_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Stack {
    pub id: String,
    pub native: bool,
    /// The branch the bottom layer targets.
    pub base: String,
    /// Bottom (on the base branch) to top.
    pub layers: Vec<StackLayer>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StackHead {
    pub number: u64,
    pub head_sha: String,
}

fn repo_key(p: &PullSummary) -> String {
    format!(
        "{}/{}/{}",
        p.repository.host, p.repository.owner, p.repository.name
    )
    .to_lowercase()
}

/// Open PRs of one repository grouped into chains: each group is ordered from
/// the layer on a plain branch upward. A branch with two PRs on top lists both
/// children in number order after their parent.
fn chains(pulls: &[&PullSummary]) -> Vec<Vec<usize>> {
    let open: Vec<usize> = (0..pulls.len())
        .filter(|&i| pulls[i].state == "open")
        .collect();
    let head = |branch: &str| {
        let found: Vec<usize> = open
            .iter()
            .copied()
            .filter(|&i| pulls[i].head_ref == branch)
            .collect();
        // Two open PRs from one branch name (forks): no reliable parent.
        (found.len() == 1).then(|| found[0])
    };
    let parent: Vec<Option<usize>> = (0..pulls.len())
        .map(|i| {
            if pulls[i].state != "open" {
                return None;
            }
            head(&pulls[i].base_ref).filter(|&p| p != i)
        })
        .collect();
    let mut groups = Vec::new();
    for &root in &open {
        if parent[root].is_some() {
            continue;
        }
        let mut order = Vec::new();
        let mut todo = vec![root];
        while let Some(at) = todo.pop() {
            if order.contains(&at) {
                continue;
            }
            order.push(at);
            let mut children: Vec<usize> = open
                .iter()
                .copied()
                .filter(|&c| parent[c] == Some(at))
                .collect();
            children.sort_by_key(|&c| std::cmp::Reverse(pulls[c].number));
            todo.extend(children);
        }
        if order.len() > 1 {
            groups.push(order);
        }
    }
    groups
}

/// Fill `stack` for open PRs that sit in an inferred stack (host stacks stay).
pub fn infer(pulls: &mut [PullSummary]) {
    let mut keys: Vec<String> = pulls.iter().map(repo_key).collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        let members: Vec<usize> = (0..pulls.len())
            .filter(|&i| repo_key(&pulls[i]) == key && pulls[i].stack.is_none())
            .collect();
        let refs: Vec<&PullSummary> = members.iter().map(|&i| &pulls[i]).collect();
        let groups = chains(&refs);
        let assigned: Vec<(usize, StackRef)> = groups
            .iter()
            .flat_map(|group| {
                let root = refs[group[0]].number;
                group.iter().enumerate().map(move |(position, &local)| {
                    (
                        local,
                        StackRef {
                            id: format!("inferred-{root}"),
                            position: position as u64 + 1,
                            size: group.len() as u64,
                            native: false,
                        },
                    )
                })
            })
            .collect();
        for (local, reference) in assigned {
            pulls[members[local]].stack = Some(reference);
        }
    }
}

fn layer(p: &PullSummary) -> StackLayer {
    StackLayer {
        number: p.number,
        url: p.url.clone(),
        title: p.title.clone(),
        state: p.state.clone(),
        draft: p.draft,
        head_ref: p.head_ref.clone(),
        head_sha: p.head_sha.clone(),
        base_ref: p.base_ref.clone(),
    }
}

/// The inferred stack holding PR `number`, from the repository's open PRs.
pub fn inferred(pulls: &[PullSummary], number: u64) -> Option<Stack> {
    let refs: Vec<&PullSummary> = pulls.iter().collect();
    chains(&refs).into_iter().find_map(|group| {
        group
            .iter()
            .any(|&i| refs[i].number == number)
            .then(|| Stack {
                id: format!("inferred-{}", refs[group[0]].number),
                native: false,
                base: refs[group[0]].base_ref.clone(),
                layers: group.iter().map(|&i| layer(refs[i])).collect(),
            })
    })
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// A GitHub stack as the REST API returns it (`stacks/{n}` or a list item).
pub(crate) fn map_native(repo: &Repository, value: &Value) -> Option<Stack> {
    let number = value["number"].as_u64()?;
    let layers = value["pull_requests"]
        .as_array()?
        .iter()
        .map(|p| {
            let n = p["number"].as_u64().unwrap_or(0);
            StackLayer {
                number: n,
                url: opt_str(&p["html_url"]).unwrap_or_else(|| repo.pull_request_url(n)),
                title: str_of(&p["title"]),
                state: if p["merged_at"].is_string() {
                    "merged".into()
                } else if p["state"] == "closed" {
                    "closed".into()
                } else {
                    "open".into()
                },
                draft: p["draft"] == true,
                head_ref: str_of(&p["head"]["ref"]),
                head_sha: str_of(&p["head"]["sha"]),
                base_ref: str_of(&p["base"]["ref"]),
            }
        })
        .collect::<Vec<_>>();
    let base = value["base"]["ref"]
        .as_str()
        .or(value["base"].as_str())
        .unwrap_or("")
        .to_owned();
    // The list endpoint leaves out bases: each layer stands on the one below.
    let mut previous = base.clone();
    let layers = layers
        .into_iter()
        .map(|mut l| {
            if l.base_ref.is_empty() {
                l.base_ref = previous.clone();
            }
            previous = l.head_ref.clone();
            l
        })
        .collect();
    Some(Stack {
        id: number.to_string(),
        native: true,
        base,
        layers,
    })
}

async fn native(host: &dyn Host, repo: &Repository, number: u64) -> Option<Stack> {
    let root = format!("repos/{}/{}/stacks", enc(&repo.owner), enc(&repo.name));
    let list = host
        .request("GET", &format!("{root}?pull_request={number}"), None)
        .await
        .ok()?;
    let id = list.as_array()?.first()?["number"].as_u64()?;
    let full = host
        .request("GET", &format!("{root}/{id}"), None)
        .await
        .ok()?;
    map_native(repo, &full)
}

/// The stack around a PR: the host's own (GitHub), else inferred from the
/// repository's open PRs. Bitbucket and Azure have no stack support here.
pub async fn read(host: &dyn Host, repo: &Repository, number: u64) -> Result<Option<Stack>> {
    if repo.kind == HostKind::Github && repo.host == "github.com" {
        if let Some(stack) = native(host, repo, number).await {
            return Ok(Some(stack));
        }
    }
    if !matches!(
        repo.kind,
        HostKind::Github | HostKind::Gitlab | HostKind::Forgejo
    ) {
        return Ok(None);
    }
    let open = list(host, repo, "open").await?;
    Ok(inferred(&open, number))
}

/// Point a PR at another base branch (the stack's base, once the layer below merged).
async fn retarget(host: &dyn Host, repo: &Repository, number: u64, base: &str) -> Result<()> {
    let (method, path, body) = match repo.kind {
        HostKind::Github | HostKind::Forgejo => (
            "PATCH",
            format!(
                "repos/{}/{}/pulls/{number}",
                enc(&repo.owner),
                enc(&repo.name)
            ),
            json!({"base":base}),
        ),
        HostKind::Gitlab => (
            "PUT",
            format!(
                "projects/{}/merge_requests/{number}",
                enc(&format!("{}/{}", repo.owner, repo.name))
            ),
            json!({"target_branch":base}),
        ),
        _ => bail!("Stacks are unavailable for this host"),
    };
    host.request(method, &path, Some(body)).await?;
    Ok(())
}

/// The layers a merge of `target` covers: every layer up to it that is not
/// merged yet. Refuses closed or draft layers in the way.
pub fn merge_range(stack: &Stack, target: u64) -> Result<Vec<StackLayer>> {
    let index = stack
        .layers
        .iter()
        .position(|l| l.number == target)
        .context("conflict: the stack changed; reload it")?;
    ensure!(stack.layers[index].state == "open", "This PR is not open");
    let range: Vec<StackLayer> = stack.layers[..=index]
        .iter()
        .filter(|l| l.state != "merged")
        .cloned()
        .collect();
    if let Some(closed) = range.iter().find(|l| l.state == "closed") {
        bail!(
            "PR #{} below is closed; reopen or unstack it first",
            closed.number
        );
    }
    if let Some(draft) = range.iter().find(|l| l.draft) {
        bail!(
            "PR #{} is a draft; mark it ready for review first",
            draft.number
        );
    }
    Ok(range)
}

/// Every open layer the caller reviewed must still be at the head they saw.
pub fn guard_heads(layers: &[StackLayer], expected: &[StackHead]) -> Result<()> {
    for layer in layers {
        let seen = expected
            .iter()
            .find(|e| e.number == layer.number)
            .with_context(|| {
                format!(
                    "conflict: the stack changed (PR #{} is new); reload it",
                    layer.number
                )
            })?;
        ensure!(
            !layer.head_sha.is_empty() && seen.head_sha == layer.head_sha,
            "conflict: PR #{} moved; reload the stack before merging or rebasing",
            layer.number
        );
    }
    Ok(())
}

fn is_transient(error: &anyhow::Error) -> bool {
    let text = error.to_string();
    text.contains("HTTP 405") || text.contains("still be checking") || text.contains("HTTP 406")
}

/// Merge one layer with its head guard, waiting out a host that is still
/// computing mergeability after a retarget or push (about a minute at most).
async fn merge_layer(
    host: &dyn Host,
    repo: &Repository,
    number: u64,
    method: &str,
    head: &str,
) -> Result<()> {
    let mut last = None;
    for attempt in 0..15 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        }
        let detail = detail(host, repo, number).await?;
        let input = MergeInput {
            method,
            expected_head: head,
            remove_credits: false,
        };
        match merge(host, repo, &detail, &input).await {
            Ok(_) => return Ok(()),
            Err(error) if is_transient(&error) => last = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("The host did not merge PR #{number}")))
}

/// GitHub's own stack merge: one request merges the layer and every open
/// layer below it, through the merge queue when the base requires one.
async fn merge_native(
    host: &dyn Host,
    repo: &Repository,
    target: &StackLayer,
    method: &str,
) -> Result<Value> {
    let item = format!(
        "repos/{}/{}/pulls/{}/merge-async",
        enc(&repo.owner),
        enc(&repo.name),
        target.number
    );
    let mut result = host
        .request(
            "PUT",
            &item,
            Some(json!({"merge_method":method,"merge_action":"default","sha":target.head_sha})),
        )
        .await?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut wait = 1;
    while result["status"] == "pending" && std::time::Instant::now() < deadline {
        let uuid = result["details"]["uuid"]
            .as_str()
            .context("Merge request id missing")?;
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        wait = (wait * 2).min(10);
        result = host
            .request("GET", &format!("{item}/{}", enc(uuid)), None)
            .await?;
    }
    match result["status"].as_str() {
        Some("merged") => Ok(json!({"status":"merged"})),
        Some("enqueued") => Ok(json!({"status":"enqueued"})),
        Some("pending") => Ok(json!({"status":"pending"})),
        _ => bail!(
            "GitHub refused the stack merge: {}",
            bounded(
                result["details"]["message"]
                    .as_str()
                    .unwrap_or("check the branch rules"),
                300
            )
        ),
    }
}

pub struct MergeStack<'a> {
    pub stack: &'a Stack,
    pub target: u64,
    pub method: &'a str,
    pub expected: &'a [StackHead],
    pub local: Option<&'a KnownRepository>,
}

/// Merge the selected layer and every open layer below it.
pub async fn merge_stack(
    host: &dyn Host,
    repo: &Repository,
    input: &MergeStack<'_>,
) -> Result<Value> {
    ensure!(
        matches!(input.method, "merge" | "squash" | "rebase"),
        "Merge method must be merge, squash or rebase"
    );
    let mut range = merge_range(input.stack, input.target)?;
    // Fresh heads from the host, then the caller's view must match all of them.
    for layer in &mut range {
        layer.head_sha = head(host, repo, layer.number).await?;
    }
    guard_heads(&range, input.expected)?;
    if input.stack.native && repo.kind == HostKind::Github {
        let result = merge_native(
            host,
            repo,
            range.last().context("Nothing to merge")?,
            input.method,
        )
        .await?;
        return Ok(
            json!({"merged": range.iter().map(|l| l.number).collect::<Vec<_>>(), "result": result}),
        );
    }
    ensure!(
        matches!(
            repo.kind,
            HostKind::Github | HostKind::Gitlab | HostKind::Forgejo
        ),
        "Stacks are unavailable for this host"
    );
    let restack = input.method != "merge" && range.len() > 1;
    ensure!(
        !restack || input.local.is_some(),
        "Squash or rebase merges of a stack restack the layers above; add a workspace with this repository, or merge with merge commits"
    );
    let above: Vec<StackLayer> = {
        let index = input
            .stack
            .layers
            .iter()
            .position(|l| l.number == input.target)
            .unwrap_or(0);
        input.stack.layers[index + 1..]
            .iter()
            .filter(|l| l.state == "open")
            .cloned()
            .collect()
    };
    let in_range: Vec<u64> = range.iter().map(|l| l.number).collect();
    let mut pending: Vec<StackLayer> = range;
    pending.extend(above);
    let mut merged = Vec::new();
    while pending
        .first()
        .is_some_and(|l| in_range.contains(&l.number))
    {
        let layer = pending.remove(0);
        if layer.base_ref != input.stack.base {
            retarget(host, repo, layer.number, &input.stack.base).await?;
        }
        merge_layer(host, repo, layer.number, input.method, &layer.head_sha)
            .await
            .with_context(|| {
                format!(
                    "Stopped at PR #{} after merging {} layers",
                    layer.number,
                    merged.len()
                )
            })?;
        merged.push(layer.number);
        if input.method != "merge" && !pending.is_empty() {
            // The layer's commits now sit in the base as new commits: replay the
            // layers above without them, so the next merge holds only its own.
            // Their PR bases stay; the next one is retargeted when its turn comes.
            let local = input
                .local
                .context("A local checkout is needed to restack")?;
            let heads = cascade(
                local,
                repo,
                &input.stack.base,
                &pending,
                Some(&layer.head_sha),
            )
            .with_context(|| format!("Merged {merged:?}; restacking the layers above stopped"))?;
            for (layer, (_, head)) in pending.iter_mut().zip(heads) {
                layer.head_sha = head;
            }
        }
    }
    Ok(json!({"merged": merged}))
}

/// Rebase every open layer onto the stack's base, bottom to top, and push
/// each branch with a lease on the head the caller saw.
pub async fn rebase_stack(
    host: &dyn Host,
    repo: &Repository,
    stack: &Stack,
    expected: &[StackHead],
    local: &KnownRepository,
) -> Result<Value> {
    let mut open: Vec<StackLayer> = stack
        .layers
        .iter()
        .filter(|l| l.state == "open")
        .cloned()
        .collect();
    ensure!(!open.is_empty(), "No open layers to rebase");
    for layer in &mut open {
        layer.head_sha = head(host, repo, layer.number).await?;
    }
    guard_heads(&open, expected)?;
    // The last merged layer below the bottom open one (squash leaves its old head outside the base).
    let merged_below = stack
        .layers
        .iter()
        .take_while(|l| l.number != open[0].number)
        .filter(|l| l.state == "merged")
        .last()
        .map(|l| l.head_sha.clone());
    let heads = cascade(local, repo, &stack.base, &open, merged_below.as_deref())?;
    // Layers whose branch below merged now target the stack's base.
    for (index, layer) in open.iter().enumerate() {
        let below = if index == 0 {
            stack.base.clone()
        } else {
            open[index - 1].head_ref.clone()
        };
        if layer.base_ref != below {
            retarget(host, repo, layer.number, &below).await?;
        }
    }
    Ok(
        json!({"rebased": heads.iter().map(|(n, h)| json!({"number":n,"headSha":h})).collect::<Vec<_>>()}),
    )
}

/// The cascade itself: fetch base and heads into a shared clone of the local
/// repository, replay each layer onto the new head of the one below, push.
pub(crate) fn cascade(
    local: &KnownRepository,
    repo: &Repository,
    base: &str,
    layers: &[StackLayer],
    merged_below: Option<&str>,
) -> Result<Vec<(u64, String)>> {
    let root = local
        .root
        .clone()
        .context("No local checkout of this repository")?;
    let remote = match &local.remote {
        Some(name) => git(&root, &["remote", "get-url", name])?.trim().to_owned(),
        None => format!("https://{}/{}/{}.git", repo.host, repo.owner, repo.name),
    };
    Repository::parse(&remote)?;
    replay(&root, &remote, base, layers, merged_below)
}

/// The replay and push, given the local repository and the remote's URL.
pub(crate) fn replay(
    root: &Path,
    remote: &str,
    base: &str,
    layers: &[StackLayer],
    merged_below: Option<&str>,
) -> Result<Vec<(u64, String)>> {
    ensure!(valid_ref_name(base), "Unusual base branch name");
    for layer in layers {
        ensure!(
            valid_ref_name(&layer.head_ref),
            "Unusual branch name for PR #{}",
            layer.number
        );
    }
    let dir = tempfile::Builder::new()
        .prefix("pockymoe-stack-")
        .tempdir()?;
    let work = dir.path().join("work");
    let source = root.to_string_lossy().into_owned();
    let target = work.to_string_lossy().into_owned();
    git(
        Path::new("/"),
        &[
            "clone",
            "--shared",
            "--no-checkout",
            "--quiet",
            "--",
            &source,
            &target,
        ],
    )?;
    let mut refspecs = vec![format!("+refs/heads/{base}:refs/stack/base")];
    for (index, layer) in layers.iter().enumerate() {
        refspecs.push(format!("+refs/heads/{}:refs/stack/{index}", layer.head_ref));
    }
    let mut args = vec!["fetch", "--quiet", "--no-tags", remote];
    args.extend(refspecs.iter().map(String::as_str));
    git(&work, &args)?;
    for (index, layer) in layers.iter().enumerate() {
        let fetched = git(&work, &["rev-parse", &format!("refs/stack/{index}")])?;
        ensure!(
            fetched.trim() == layer.head_sha,
            "conflict: PR #{}'s branch moved; reload the stack",
            layer.number
        );
    }
    let identity: Vec<String> =
        if git(&work, &["config", "user.email"]).is_ok_and(|e| !e.trim().is_empty()) {
            Vec::new()
        } else {
            vec![
                "-c".into(),
                "user.name=Pockymoe".into(),
                "-c".into(),
                "user.email=pockymoe@localhost".into(),
            ]
        };
    let with_identity = |rest: &[&str]| -> Vec<String> {
        identity
            .iter()
            .cloned()
            .chain(rest.iter().map(|s| s.to_string()))
            .collect()
    };
    let mut parent_old = "refs/stack/base".to_owned();
    let mut parent_new = git(&work, &["rev-parse", "refs/stack/base"])?
        .trim()
        .to_owned();
    let mut heads = Vec::new();
    for (index, layer) in layers.iter().enumerate() {
        let upstream = if index == 0 {
            match merged_below.filter(|sha| {
                git(
                    &work,
                    &["merge-base", "--is-ancestor", sha, &layer.head_sha],
                )
                .is_ok()
            }) {
                Some(sha) => sha.to_owned(),
                None => git(&work, &["merge-base", &parent_old, &layer.head_sha])?
                    .trim()
                    .to_owned(),
            }
        } else {
            parent_old.clone()
        };
        git(&work, &["checkout", "--quiet", "--detach", &layer.head_sha])?;
        let args = with_identity(&["rebase", "--quiet", "--onto", &parent_new, &upstream]);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        if git(&work, &refs).is_err() {
            let _ = git(&work, &["rebase", "--abort"]);
            bail!(
                "PR #{} conflicts with the layer below it; {} layers were rebased and pushed. Resolve it in a worktree and push",
                layer.number,
                index
            );
        }
        let rebased = git(&work, &["rev-parse", "HEAD"])?.trim().to_owned();
        if rebased != layer.head_sha {
            let lease = format!(
                "--force-with-lease=refs/heads/{}:{}",
                layer.head_ref, layer.head_sha
            );
            let spec = format!("{rebased}:refs/heads/{}", layer.head_ref);
            git(&work, &["push", "--quiet", &lease, remote, &spec]).with_context(|| {
                format!("Pushing PR #{} failed after {} layers", layer.number, index)
            })?;
        }
        parent_old = layer.head_sha.clone();
        parent_new = rebased.clone();
        heads.push((layer.number, rebased));
    }
    Ok(heads)
}

/// After creating a PR on top of another: join GitHub's stack, or start one.
pub async fn join_native(
    host: &dyn Host,
    repo: &Repository,
    below: u64,
    created: u64,
) -> Result<Value> {
    ensure!(
        repo.kind == HostKind::Github && repo.host == "github.com",
        "Host stacks are a GitHub feature"
    );
    let root = format!("repos/{}/{}/stacks", enc(&repo.owner), enc(&repo.name));
    let list = host
        .request("GET", &format!("{root}?pull_request={below}"), None)
        .await?;
    if let Some(id) = list
        .as_array()
        .and_then(|a| a.first())
        .and_then(|s| s["number"].as_u64())
    {
        host.request(
            "POST",
            &format!("{root}/{id}/add"),
            Some(json!({"pull_requests":[created]})),
        )
        .await
    } else {
        host.request(
            "POST",
            &root,
            Some(json!({"pull_requests":[below, created]})),
        )
        .await
    }
}
