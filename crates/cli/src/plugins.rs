use anyhow::{Context, Result};
use clap::Subcommand;
use serde_json::{json, Value};

#[derive(Subcommand)]
pub enum PluginCommand {
    /// List built-in and installed plugins.
    List,
    /// Clone a git repository with pockymoe-plugin.json and register it disabled.
    Install {
        /// Git URL or path, e.g. https://github.com/owner/plugin.git
        source: String,
        /// Branch or tag to clone.
        #[arg(long = "ref")]
        git_ref: Option<String>,
    },
    /// Fast-forward an installed plugin and rerun its setup.
    Update {
        id: String,
    },
    /// Uninstall a plugin; its data directory stays unless --purge-data.
    Remove {
        id: String,
        #[arg(long)]
        purge_data: bool,
    },
    /// Turn a plugin on for this device. Read its permissions first (`list`).
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    /// Run an installed plugin's action at device scope and print its output.
    Run {
        id: String,
        action: String,
        /// Action input as JSON.
        #[arg(long, default_value = "{}")]
        input: String,
    },
    /// Recent device-scope runs of an installed plugin.
    Runs {
        id: String,
    },
}

impl crate::threads::Client {
    pub async fn plugin(&self, command: PluginCommand) -> Result<Value> {
        let (value, wait) = match command {
            PluginCommand::List => (json!({"operation":"pluginList"}), 0),
            PluginCommand::Install { source, git_ref } => (
                json!({"operation":"pluginInstall","source":source,"ref":git_ref}),
                600,
            ),
            PluginCommand::Update { id } => {
                (json!({"operation":"pluginUpdate","pluginId":id}), 600)
            }
            PluginCommand::Remove { id, purge_data } => (
                json!({"operation":"pluginRemove","pluginId":id,"purgeData":purge_data}),
                0,
            ),
            PluginCommand::Enable { id } => (
                json!({"operation":"pluginEnable","pluginId":id,"enabled":true}),
                0,
            ),
            PluginCommand::Disable { id } => (
                json!({"operation":"pluginEnable","pluginId":id,"enabled":false}),
                0,
            ),
            PluginCommand::Run { id, action, input } => {
                let input: Value = serde_json::from_str(&input).context("--input is not JSON")?;
                (
                    json!({"operation":"pluginRun","pluginId":id,"action":action,"input":input}),
                    300,
                )
            }
            PluginCommand::Runs { id } => (json!({"operation":"pluginRuns","pluginId":id}), 0),
        };
        self.request_for(value, wait).await
    }
}
