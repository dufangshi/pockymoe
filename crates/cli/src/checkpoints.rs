use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::{json, Value};
#[derive(Args)]
pub struct CheckpointArgs {
    #[arg(long, global = true, default_value = "self")]
    pub thread: String,
    #[arg(long, global = true)]
    pub working_copy_id: Option<String>,
    #[command(subcommand)]
    pub command: CheckpointCommand,
}
#[derive(Subcommand)]
pub enum CheckpointCommand {
    List,
    Diff {
        turn_id: String,
    },
    Restore {
        turn_id: String,
        #[arg(long,default_value="start",value_parser=["start","end"])]
        phase: String,
        #[arg(long)]
        preview: bool,
    },
    Undo {
        undo_id: String,
    },
}
impl crate::threads::Client {
    pub async fn checkpoint(&self, args: CheckpointArgs) -> Result<Value> {
        let mut input = match args.command {
            CheckpointCommand::List => json!({"operation":"checkpointList"}),
            CheckpointCommand::Diff { turn_id } => {
                json!({"operation":"checkpointDiff","turnId":turn_id})
            }
            CheckpointCommand::Restore {
                turn_id,
                phase,
                preview,
            } => {
                json!({"operation":"checkpointRestore","turnId":turn_id,"phase":phase,"preview":preview})
            }
            CheckpointCommand::Undo { undo_id } => {
                json!({"operation":"checkpointUndo","undoId":undo_id})
            }
        };
        input["threadId"] = json!(self.id(&args.thread).await?);
        input["fromThreadId"] = json!(self.from);
        input["workingCopyId"] = json!(args.working_copy_id);
        self.request(input).await
    }
}
