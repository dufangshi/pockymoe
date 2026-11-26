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
        #[arg(long, default_value = "main")]
        base: String,
        #[arg(long)]
        draft: bool,
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
            } => {
                json!({"action":"create","title":title,"body":body,"head":head,"base":base,"draft":draft})
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
        }
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
        input["kind"] = json!(if issue { "issue" } else { "pr" });
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
}
