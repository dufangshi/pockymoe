use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::{json, Value};
#[derive(Args)]
#[command(
    after_long_help = "Uses the local thread's working copy and owner's host sign-in. More: pockymoe guide pr"
)]
pub struct SourceControlArgs {
    #[arg(long, global = true, default_value = "self")]
    pub thread: String,
    /// Select an item already linked to this thread (including another repository).
    #[arg(long, global = true)]
    pub item_url: Option<String>,
    /// Allow an automatic pre-create push while the thread is running.
    #[arg(long, global = true)]
    pub confirm_running: bool,
    #[command(subcommand)]
    pub command: Command,
}
#[derive(Subcommand)]
pub enum Command {
    Publish {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "github.com")]
        host: String,
        #[arg(long)]
        public: bool,
    },
    Cleanup,
    List {
        #[arg(long)]
        search: Option<String>,
    },
    Show {
        number: u64,
    },
    Create {
        #[arg(long)]
        title: String,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        head: Option<String>,
        #[arg(
            long,
            default_value = "main",
            default_value_if("stack_on", clap::builder::ArgPredicate::IsPresent, None)
        )]
        base: Option<String>,
        #[arg(long)]
        draft: bool,
        /// Stack the new PR on top of open PR NUMBER (its base becomes that PR's head branch).
        #[arg(long, conflicts_with = "base")]
        stack_on: Option<u64>,
    },
    Update {
        number: u64,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        body: Option<String>,
    },
    Comment {
        number: u64,
        #[arg(long)]
        body: String,
    },
    Merge {
        number: u64,
        #[arg(long)]
        expected_head: String,
        #[arg(long,default_value="squash",value_parser=["squash","merge","rebase"])]
        method: String,
    },
    Checks {
        number: u64,
        #[arg(long)]
        head_sha: Option<String>,
    },
    Watch {
        number: u64,
    },
    Unwatch,
    Link {
        url: String,
    },
    Unlink {
        url: String,
    },
    /// Fetch a PR head (including forks) and create a managed worktree.
    WorkOn {
        number: u64,
        #[arg(long)]
        name: Option<String>,
    },
    Comments {
        number: u64,
    },
    Reviews {
        number: u64,
    },
    Files {
        number: u64,
    },
    Commits {
        number: u64,
    },
    Ready {
        number: u64,
        #[arg(long)]
        node_id: Option<String>,
        #[arg(long)]
        title: Option<String>,
    },
    AutoMerge {
        number: u64,
        #[arg(long)]
        node_id: Option<String>,
        #[arg(long)]
        expected_head: Option<String>,
    },
    /// Review a PR: read its state, draft line comments, submit, reply, resolve.
    #[command(
        after_long_help = "Drafts stay on this device until `review submit`. Submit refuses if the PR head moved since the SHA you pass. More: pockymoe guide pr"
    )]
    Review {
        #[command(subcommand)]
        command: ReviewCommand,
    },
}

#[derive(Subcommand)]
pub enum ReviewCommand {
    /// Conversation, reviews, threads, checks and your drafts, in one JSON.
    Show { number: u64 },
    /// Changed files with patches and viewed state.
    Files { number: u64 },
    /// Add a draft line comment (or a file comment with --file).
    Comment {
        number: u64,
        #[arg(long)]
        path: String,
        /// Last line of the range (new-file numbers unless --side left).
        #[arg(long, required_unless_present = "file")]
        line: Option<u64>,
        #[arg(long)]
        start_line: Option<u64>,
        #[arg(long, default_value = "right", value_parser = ["left", "right"])]
        side: String,
        /// Comment on the whole file.
        #[arg(long)]
        file: bool,
        #[arg(long)]
        body: String,
    },
    /// List your draft comments.
    Drafts { number: u64 },
    /// Discard one draft (--id) or all of them (--all).
    Discard {
        number: u64,
        #[arg(long, conflicts_with = "all")]
        id: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Submit your drafts as one review.
    Submit {
        number: u64,
        #[arg(long, default_value = "comment", value_parser = ["comment", "approve", "request-changes"])]
        event: String,
        #[arg(long)]
        body: Option<String>,
        /// The head SHA you reviewed (from `review show`).
        #[arg(long)]
        expected_head: String,
    },
    /// Reply in a review thread.
    Reply {
        number: u64,
        #[arg(long)]
        thread: String,
        #[arg(long)]
        body: String,
    },
    /// Resolve (or with --unresolve reopen) a review thread.
    Resolve {
        number: u64,
        #[arg(long)]
        thread: String,
        #[arg(long)]
        unresolve: bool,
    },
    /// Mark files viewed (or --unviewed).
    Viewed {
        number: u64,
        paths: Vec<String>,
        #[arg(long)]
        unviewed: bool,
    },
    /// Request or remove reviewers (`org/team` for teams).
    Reviewers {
        number: u64,
        #[arg(long)]
        add: Vec<String>,
        #[arg(long)]
        remove: Vec<String>,
    },
    /// close, reopen, draft (convert to draft) or ready.
    State {
        number: u64,
        #[arg(value_parser = ["close", "reopen", "draft", "ready"])]
        state: String,
    },
    /// Merge the reviewed head, optionally without agent co-author credits.
    Merge {
        number: u64,
        #[arg(long)]
        expected_head: String,
        #[arg(long, default_value = "squash", value_parser = ["squash", "merge", "rebase"])]
        method: String,
        #[arg(long)]
        remove_agent_credits: bool,
    },
    /// Rerun failed jobs of a check (token from `review show`).
    Rerun {
        number: u64,
        #[arg(long)]
        token: String,
    },
    /// Open a PR that reverts a merged GitHub PR.
    Revert { number: u64 },
    /// The stack around a PR: layers bottom to top with their heads.
    Stack { number: u64 },
    /// Merge this PR and every open layer below it.
    #[command(
        after_long_help = "Pass the heads you reviewed for every open layer up to NUMBER, from `review stack`: --expected-heads 12:SHA,13:SHA. A moved layer refuses the merge."
    )]
    MergeStack {
        number: u64,
        #[arg(long, value_delimiter = ',')]
        expected_heads: Vec<String>,
        #[arg(long, default_value = "merge", value_parser = ["merge", "squash", "rebase"])]
        method: String,
    },
    /// Rebase every open layer onto the stack's base and force-push each branch
    /// with a lease. Rewrites history; checks start again.
    RebaseStack {
        number: u64,
        #[arg(long, value_delimiter = ',')]
        expected_heads: Vec<String>,
    },
    /// Write the review guide (or --show the stored one).
    Guide {
        number: u64,
        #[arg(long)]
        show: bool,
    },
}
impl ReviewCommand {
    fn request(self) -> Value {
        let (action, number, mut v) = match self {
            Self::Show { number } => ("detail", number, json!({})),
            Self::Files { number } => ("files", number, json!({})),
            Self::Comment {
                number,
                path,
                line,
                start_line,
                side,
                file,
                body,
            } => (
                "draftSave",
                number,
                json!({"kind":if file {"file"} else {"line"},"path":path,"line":line,"startLine":start_line,"side":side,"body":body}),
            ),
            Self::Drafts { number } => ("drafts", number, json!({})),
            Self::Discard { number, id, all } => {
                ("draftDelete", number, json!({"id":id,"all":all}))
            }
            Self::Submit {
                number,
                event,
                body,
                expected_head,
            } => (
                "submit",
                number,
                json!({"event":if event == "request-changes" {"requestChanges".to_owned()} else {event},"body":body,"expectedHead":expected_head}),
            ),
            Self::Reply {
                number,
                thread,
                body,
            } => ("reply", number, json!({"threadId":thread,"body":body})),
            Self::Resolve {
                number,
                thread,
                unresolve,
            } => (
                "resolve",
                number,
                json!({"threadId":thread,"resolved":!unresolve}),
            ),
            Self::Viewed {
                number,
                paths,
                unviewed,
            } => ("viewed", number, json!({"paths":paths,"viewed":!unviewed})),
            Self::Reviewers {
                number,
                add,
                remove,
            } => ("reviewers", number, json!({"add":add,"remove":remove})),
            Self::State { number, state } => ("state", number, json!({"state":state})),
            Self::Merge {
                number,
                expected_head,
                method,
                remove_agent_credits,
            } => (
                "merge",
                number,
                json!({"expectedHead":expected_head,"method":method,"removeAgentCredits":remove_agent_credits}),
            ),
            Self::Rerun { number, token } => ("rerun", number, json!({"token":token})),
            Self::Revert { number } => ("revert", number, json!({})),
            Self::Stack { number } => ("stack", number, json!({})),
            Self::MergeStack {
                number,
                expected_heads,
                method,
            } => (
                "mergeStack",
                number,
                json!({"method":method,"expectedHeads":stack_heads(&expected_heads)}),
            ),
            Self::RebaseStack {
                number,
                expected_heads,
            } => (
                "rebaseStack",
                number,
                json!({"expectedHeads":stack_heads(&expected_heads)}),
            ),
            Self::Guide { number, show } => {
                (if show { "guideShow" } else { "guide" }, number, json!({}))
            }
        };
        v["action"] = json!("review");
        v["reviewAction"] = json!(action);
        v["number"] = json!(number);
        v
    }
}
impl Command {
    fn request(self) -> Value {
        match self {
            Self::Publish { name, host, public } => {
                json!({"action":"publish","name":name,"host":host,"private":!public})
            }
            Self::Cleanup => json!({"action":"cleanup"}),
            Self::List { search } => json!({"action":"list","search":search}),
            Self::Show { number } => json!({"action":"show","number":number}),
            Self::Create {
                title,
                body,
                head,
                base,
                draft,
                stack_on,
            } => {
                json!({"action":"create","title":title,"body":body,"head":head,"base":base,"draft":draft,"stackOn":stack_on})
            }
            Self::Update {
                number,
                title,
                body,
            } => {
                let mut v = json!({"action":"update","number":number});
                if let Some(title) = title {
                    v["title"] = json!(title)
                }
                if let Some(body) = body {
                    v["body"] = json!(body)
                }
                v
            }
            Self::Comment { number, body } => {
                json!({"action":"comment","number":number,"body":body})
            }
            Self::Merge {
                number,
                expected_head,
                method,
            } => {
                json!({"action":"merge","number":number,"expectedHead":expected_head,"method":method})
            }
            Self::Checks { number, head_sha } => {
                json!({"action":"checks","number":number,"headSha":head_sha})
            }
            Self::Watch { number } => json!({"action":"watch","number":number}),
            Self::Unwatch => json!({"action":"unwatch"}),
            Self::Link { url } => json!({"action":"link","url":url}),
            Self::Unlink { url } => json!({"action":"unlink","url":url}),
            Self::WorkOn { number, name } => json!({"action":"workOn","number":number,"name":name}),
            Self::Comments { number } => json!({"action":"comments","number":number}),
            Self::Reviews { number } => json!({"action":"reviews","number":number}),
            Self::Files { number } => json!({"action":"files","number":number}),
            Self::Commits { number } => json!({"action":"commits","number":number}),
            Self::Ready {
                number,
                node_id,
                title,
            } => json!({"action":"ready","number":number,"nodeId":node_id,"title":title}),
            Self::AutoMerge {
                number,
                node_id,
                expected_head,
            } => {
                json!({"action":"autoMerge","number":number,"nodeId":node_id,"expectedHead":expected_head})
            }
            Self::Review { command } => command.request(),
        }
    }
}
/// `12:SHA` pairs into `[{number, headSha}]`; malformed entries are sent as
/// given so the Supervisor refuses them with a reason.
fn stack_heads(pairs: &[String]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|pair| match pair.split_once(':') {
                Some((number, sha)) => {
                    json!({"number":number.trim().parse::<u64>().ok(),"headSha":sha.trim()})
                }
                None => json!({"number":null,"headSha":pair}),
            })
            .collect(),
    )
}

/// PR or issue for item commands. Review actions use `kind` for the draft kind
/// (line, file, summary), so it is left as the command set it.
fn item_kind(input: &mut Value, issue: bool) {
    if input["action"] != "review" {
        input["kind"] = json!(if issue { "issue" } else { "pr" });
    }
}
impl crate::threads::Client {
    pub async fn source_control(&self, args: SourceControlArgs, issue: bool) -> Result<Value> {
        let mut input = args.command.request();
        input["operation"] = json!("sourceControl");
        input["confirmRunning"] = json!(args.confirm_running);
        if let Some(url) = args.item_url {
            input["url"] = json!(url);
        }
        input["threadId"] = json!(self.id(&args.thread).await?);
        input["fromThreadId"] = json!(self.from);
        item_kind(&mut input, issue);
        self.request(input).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merge_keeps_expected_head() {
        let v = Command::Merge {
            number: 42,
            expected_head: "abc".into(),
            method: "squash".into(),
        }
        .request();
        assert_eq!(v["expectedHead"], "abc");
        assert_eq!(v["number"], 42);
    }
    #[test]
    fn review_commands_map_to_review_actions() {
        let v = Command::Review {
            command: ReviewCommand::Submit {
                number: 7,
                event: "request-changes".into(),
                body: Some("Please split this".into()),
                expected_head: "abc".into(),
            },
        }
        .request();
        assert_eq!(v["action"], "review");
        assert_eq!(v["reviewAction"], "submit");
        assert_eq!(v["event"], "requestChanges");
        assert_eq!(v["expectedHead"], "abc");
        let v = Command::Review {
            command: ReviewCommand::Comment {
                number: 7,
                path: "src/a.rs".into(),
                line: Some(12),
                start_line: Some(10),
                side: "right".into(),
                file: false,
                body: "Why here?".into(),
            },
        }
        .request();
        assert_eq!(v["reviewAction"], "draftSave");
        let mut v = v;
        item_kind(&mut v, false);
        assert_eq!(
            v["kind"], "line",
            "the draft kind survives the item scoping"
        );
        assert_eq!(v["startLine"], 10);
        let mut show = Command::Show { number: 1 }.request();
        item_kind(&mut show, true);
        assert_eq!(show["kind"], "issue");
        let v = Command::Review {
            command: ReviewCommand::MergeStack {
                number: 13,
                expected_heads: vec!["12:aaa".into(), "13:bbb".into()],
                method: "squash".into(),
            },
        }
        .request();
        assert_eq!(v["reviewAction"], "mergeStack");
        assert_eq!(
            v["expectedHeads"],
            json!([{"number":12,"headSha":"aaa"},{"number":13,"headSha":"bbb"}])
        );
    }
}
