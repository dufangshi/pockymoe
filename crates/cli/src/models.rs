use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde_json::{json, Value};

#[derive(Subcommand)]
pub enum ModelCommand {
    /// Saved connections you can call, their models, and the base URL for each SDK.
    List,
    /// Ask one question and print the answer text.
    Ask {
        /// Connection id or name, from `model list`.
        connection: String,
        /// The question; `-` reads it from stdin.
        text: String,
        /// A model the connection serves; default is the connection's own.
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        system: Option<String>,
        #[arg(long, default_value_t = 1024)]
        max_tokens: u64,
    },
    /// Send one raw API request and print the upstream's response body.
    Request {
        connection: String,
        /// Route in the connection's wire format, e.g. v1/chat/completions,
        /// v1/responses, v1/messages, v1/models.
        path: String,
        /// Request body as JSON; `-` reads stdin. Without a body the request is a GET.
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        #[arg(long)]
        body_file: Option<std::path::PathBuf>,
    },
    /// Owner only: let this device's agents call a connection.
    Allow { connection: String },
    /// Owner only: stop agents from calling a connection.
    Deny { connection: String },
}

fn stdin_or(text: String) -> Result<String> {
    if text != "-" {
        return Ok(text);
    }
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)?;
    Ok(input)
}

impl crate::threads::Client {
    /// Prints what the command returns; a raw request prints the upstream's body as is.
    pub async fn model(&self, command: ModelCommand) -> Result<()> {
        let value = match command {
            ModelCommand::List => self.request(json!({"operation":"modelList"})).await?,
            ModelCommand::Ask {
                connection,
                text,
                model,
                system,
                max_tokens,
            } => {
                let answer = self
                    .request_for(
                        json!({"operation":"modelComplete","connectionId":connection,"user":stdin_or(text)?,
                            "system":system.unwrap_or_default(),"model":model.unwrap_or_default(),"maxTokens":max_tokens}),
                        120,
                    )
                    .await?;
                println!("{}", answer["text"].as_str().context("no answer text")?);
                return Ok(());
            }
            ModelCommand::Request {
                connection,
                path,
                body,
                body_file,
            } => {
                let body = match (body, body_file) {
                    (Some(body), _) => Some(stdin_or(body)?),
                    (None, Some(file)) => Some(
                        std::fs::read_to_string(&file)
                            .with_context(|| format!("cannot read {}", file.display()))?,
                    ),
                    (None, None) => None,
                };
                if let Some(body) = &body {
                    serde_json::from_str::<Value>(body).context("the request body is not JSON")?;
                }
                // The proxy route takes a connection id; a name is resolved first.
                let listed = self.request(json!({"operation":"modelList"})).await?;
                let id = listed["connections"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|c| {
                        c["id"] == connection.as_str()
                            || c["name"]
                                .as_str()
                                .is_some_and(|name| name.eq_ignore_ascii_case(&connection))
                    })
                    .and_then(|c| c["id"].as_str())
                    .with_context(|| {
                        format!("no connection named {connection}; see `pockymoe model list`")
                    })?
                    .to_owned();
                let (status, text) = self.model_proxy(&id, &path, body).await?;
                println!("{text}");
                if !(200..300).contains(&status) {
                    bail!("HTTP {status}");
                }
                return Ok(());
            }
            ModelCommand::Allow { connection } => {
                self.request(
                    json!({"operation":"modelAccess","connectionId":connection,"enabled":true}),
                )
                .await?
            }
            ModelCommand::Deny { connection } => {
                self.request(
                    json!({"operation":"modelAccess","connectionId":connection,"enabled":false}),
                )
                .await?
            }
        };
        println!("{}", serde_json::to_string_pretty(&value)?);
        Ok(())
    }
}
