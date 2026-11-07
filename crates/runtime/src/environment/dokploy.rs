//! Dokploy v0.30.6 REST procedures (no tRPC envelopes).
use std::time::Duration;

use anyhow::{ensure, Result};
use reqwest::{header::HeaderValue, Client, Method};
use serde_json::{json, Value};

use super::ConnectionRecord;

pub(super) struct DokployClient {
    client: Client,
    base_url: String,
    key: HeaderValue,
}

#[derive(Debug, thiserror::Error)]
#[error("Dokploy {procedure} returned HTTP {status}")]
struct HttpFailure {
    procedure: String,
    status: u16,
}

impl DokployClient {
    pub fn new(connection: &ConnectionRecord) -> Result<Self> {
        let mut key = HeaderValue::from_str(&connection.api_key)
            .map_err(|_| anyhow::anyhow!("invalid Dokploy API credential"))?;
        key.set_sensitive(true);
        Ok(Self {
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .user_agent("pockymoe-environment/1.0")
                .build()
                .map_err(|_| anyhow::anyhow!("Dokploy HTTP client could not be initialized"))?,
            base_url: connection.public.base_url.trim_end_matches('/').to_owned(),
            key,
        })
    }

    async fn request(&self, method: Method, procedure: &str, payload: Value) -> Result<Value> {
        let request = self
            .client
            .request(method.clone(), format!("{}/api/{procedure}", self.base_url))
            .header("x-api-key", self.key.clone());
        let request = if method == Method::GET {
            request.query(&payload)
        } else {
            request.json(&payload)
        };
        let mut response = request
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Dokploy {procedure} transport failed"))?;
        if !response.status().is_success() {
            return Err(HttpFailure {
                procedure: procedure.into(),
                status: response.status().as_u16(),
            }
            .into());
        }
        ensure!(
            response
                .content_length()
                .is_none_or(|n| n <= 8 * 1024 * 1024),
            "Dokploy response exceeds the size limit"
        );
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("Dokploy {procedure} response could not be read"))?
        {
            ensure!(
                body.len() + chunk.len() <= 8 * 1024 * 1024,
                "Dokploy response exceeds the size limit"
            );
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body)
            .map_err(|_| anyhow::anyhow!("Dokploy {procedure} returned invalid JSON"))
    }

    pub async fn projects(&self) -> Result<Value> {
        self.request(Method::GET, "project.all", json!({})).await
    }

    pub async fn compose(&self, id: &str) -> Result<Value> {
        let value = self
            .request(Method::GET, "compose.one", json!({"composeId":id}))
            .await?;
        ensure!(
            value["composeId"] == id,
            "Dokploy Compose detail identity differs"
        );
        Ok(value)
    }

    pub async fn environment(&self, id: &str) -> Result<Value> {
        self.request(Method::GET, "environment.one", json!({"environmentId":id}))
            .await
    }

    pub async fn optional_compose(&self, id: &str) -> Result<Option<Value>> {
        match self.compose(id).await {
            Ok(value) => Ok(Some(value)),
            Err(error)
                if error
                    .downcast_ref::<HttpFailure>()
                    .is_some_and(|v| v.status == 404) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    pub async fn jobs_idle(&self, id: &str) -> Result<bool> {
        let jobs = self
            .request(Method::GET, "deployment.queueList", json!({}))
            .await?;
        let jobs = jobs
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Dokploy deployment queue response is invalid"))?;
        Ok(!jobs.iter().any(|job| job["data"]["composeId"] == id))
    }

    pub async fn create(
        &self,
        environment_id: &str,
        app_name: &str,
        compose: &str,
    ) -> Result<String> {
        let response = self.request(Method::POST, "compose.create", json!({
            "name":app_name, "appName":app_name, "environmentId":environment_id,
            "description":"Pockymoe execution environment; project and state storage are external",
            "sourceType":"raw", "composeType":"docker-compose", "composeFile":compose,
            "serverId":null
        })).await?;
        response["composeId"]
            .as_str()
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Dokploy did not return a compose identity; reconcile before retrying"
                )
            })
    }

    pub async fn update(&self, id: &str, compose: &str) -> Result<()> {
        self.request(Method::POST, "compose.update", json!({
            "composeId":id, "sourceType":"raw", "composeType":"docker-compose",
            "composeFile":compose, "autoDeploy":false, "randomize":false,
            "isolatedDeployment":false, "isolatedDeploymentsVolume":false,
            "createEnvFile":false, "env":"", "command":"",
            "serviceNetworks":[{"serviceName":"worker","networkIds":[],"detachDokployNetwork":true}]
        })).await?;
        Ok(())
    }

    pub async fn deploy(&self, id: &str) -> Result<()> {
        self.request(
            Method::POST,
            "compose.deploy",
            json!({"composeId":id,"freshVolumes":false}),
        )
        .await?;
        Ok(())
    }

    /// Dokploy v0.30.6 stop/delete omit an explicit Compose file/cwd. Execute
    /// the fixed lifecycle command through its deployment runner, which runs in
    /// the managed raw Compose code directory. Never accept a caller command.
    pub async fn stop_or_down(&self, id: &str, app_name: &str, destroy: bool) -> Result<()> {
        ensure!(
            !app_name.is_empty()
                && app_name.len() <= 63
                && app_name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "invalid managed Compose project identity"
        );
        let operation = if destroy {
            "down --remove-orphans"
        } else {
            "stop"
        };
        self.request(Method::POST,"compose.update",json!({"composeId":id,"command":format!("compose -p {app_name} -f docker-compose.yml {operation}")})).await?;
        self.deploy(id).await
    }

    pub async fn clean_queues(&self, id: &str) -> Result<()> {
        self.request(Method::POST, "compose.cleanQueues", json!({"composeId":id}))
            .await?;
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        self.request(
            Method::POST,
            "compose.delete",
            json!({"composeId":id,"deleteVolumes":false}),
        )
        .await?;
        Ok(())
    }
}

pub(super) fn find_owned_compose(
    value: &Value,
    name: &str,
    environment: &str,
) -> Result<Option<String>> {
    // v0.30.6 environment.one returns name/id summaries, without appName or
    // environmentId on each Compose. Fetch compose.one before trusting a match.
    ensure!(
        value["environmentId"] == environment,
        "Dokploy environment identity differs"
    );
    let entries = value["compose"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Dokploy environment Compose listing is unavailable"))?;
    let mut matches = entries
        .iter()
        .filter(|entry| entry["name"] == name)
        .map(|entry| {
            entry["composeId"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("Dokploy Compose listing has no identity"))
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        matches.len() <= 1,
        "multiple Dokploy resources claim the environment identity"
    );
    Ok(matches.pop())
}

pub(super) fn verify_owned_compose(
    value: &Value,
    name: &str,
    environment: &str,
    expected_app_name: Option<&str>,
) -> Result<String> {
    ensure!(
        value["name"] == name && value["environmentId"] == environment,
        "Dokploy resource ownership differs"
    );
    ensure!(
        value.get("serverId") == Some(&Value::Null),
        "remote Dokploy execution has no same-storage adapter"
    );
    ensure!(
        value["sourceType"] == "raw" && value["composeType"] == "docker-compose",
        "Dokploy resource is not the managed raw Compose application"
    );
    let app_name = value["appName"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Dokploy Compose project identity is unavailable"))?;
    ensure!(
        !app_name.is_empty()
            && app_name.len() <= 63
            && app_name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && (app_name == name
                || app_name
                    .strip_prefix(name)
                    .is_some_and(|suffix| suffix.starts_with('-') && suffix.len() > 1)),
        "Dokploy Compose project identity differs"
    );
    ensure!(
        expected_app_name.is_none_or(|expected| expected == app_name),
        "Dokploy Compose project identity changed"
    );
    Ok(app_name.to_owned())
}
