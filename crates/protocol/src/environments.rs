//! Public execution-environment wire contract. Credentials occur only in input.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EnvironmentState {
    Creating,
    Stopped,
    Starting,
    Running,
    Stopping,
    Error,
    Destroyed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentLimitsDto {
    pub cpu: f64,
    pub memory_bytes: u64,
    pub pids: u32,
    pub nofile: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentMountDto {
    /// Docker-daemon source, which may differ from the Supervisor path.
    pub source: String,
    /// The original Supervisor path, preserved inside the environment.
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDto {
    pub id: String,
    pub name: String,
    pub connection_id: String,
    pub provider: String,
    pub thread_id: String,
    pub generation: u64,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose_id: Option<String>,
    pub workspace_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    pub mounts: Vec<EnvironmentMountDto>,
    pub limits: EnvironmentLimitsDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_limits: Option<EnvironmentLimitsDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub disk_quota_supported: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentConnectionDto {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub configured: bool,
    pub dokploy_environment_id: String,
    pub image: String,
    pub storage_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_host: Option<String>,
    pub allowed_roots: Vec<String>,
    #[serde(default)]
    pub allowed_egress_hosts: Vec<String>,
    pub cpu_limit: f64,
    pub memory_limit_bytes: u64,
    pub pids_limit: u32,
    pub nofile_limit: u32,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateEnvironmentConnectionRequest {
    pub name: String,
    pub base_url: String,
    /// Write-only. Omission preserves an existing key; an empty key is rejected.
    #[serde(default, skip_serializing)]
    pub api_key: Option<String>,
    pub dokploy_environment_id: String,
    pub image: String,
    pub storage_node: String,
    #[serde(default)]
    pub docker_host: Option<String>,
    pub allowed_roots: Vec<String>,
    #[serde(default)]
    pub allowed_egress_hosts: Vec<String>,
    pub cpu_limit: f64,
    pub memory_limit_bytes: u64,
    pub pids_limit: u32,
    pub nofile_limit: u32,
}

impl std::fmt::Debug for UpdateEnvironmentConnectionRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateEnvironmentConnectionRequest")
            .field("name", &self.name)
            .field("api_key", &self.api_key.as_ref().map(|_| "[redacted]"))
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentBudgetDto {
    pub max_running: u32,
    pub max_cpu: f64,
    pub max_memory_bytes: u64,
    pub reserved_cpu: f64,
    pub reserved_memory_bytes: u64,
}

impl Default for EnvironmentBudgetDto {
    fn default() -> Self {
        Self {
            max_running: 2,
            max_cpu: 4.0,
            max_memory_bytes: 4 * 1024 * 1024 * 1024,
            reserved_cpu: 1.0,
            reserved_memory_bytes: 512 * 1024 * 1024,
        }
    }
}

pub type UpdateEnvironmentBudgetRequest = EnvironmentBudgetDto;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentsDto {
    pub connections: Vec<EnvironmentConnectionDto>,
    pub environments: Vec<EnvironmentDto>,
    pub budget: EnvironmentBudgetDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestEnvironmentConnectionResponse {
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateEnvironmentRequest {
    pub connection_id: String,
    pub thread_id: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindThreadEnvironmentRequest {
    pub environment_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadEnvironmentDto {
    pub environment: Option<EnvironmentDto>,
    pub supported_harnesses: Vec<String>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EnvironmentAction {
    Start,
    Stop,
    Rebuild,
    Destroy,
}

impl std::str::FromStr for EnvironmentAction {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "start" => Ok(Self::Start),
            "stop" => Ok(Self::Stop),
            "rebuild" => Ok(Self::Rebuild),
            "destroy" => Ok(Self::Destroy),
            _ => Err("unsupported environment action"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_key_is_write_only_and_debug_redacted() {
        let input: UpdateEnvironmentConnectionRequest = serde_json::from_value(serde_json::json!({
            "name":"test", "baseUrl":"http://127.0.0.1:3000", "apiKey":"fixture-secret",
            "dokployEnvironmentId":"test", "image":"fixture@sha256:abc", "storageNode":"local",
            "allowedRoots":["/project"], "cpuLimit":1, "memoryLimitBytes":536870912,
            "pidsLimit":256, "nofileLimit":1024
        }))
        .unwrap();
        assert!(!serde_json::to_string(&input)
            .unwrap()
            .contains("fixture-secret"));
        assert!(!format!("{input:?}").contains("fixture-secret"));
        assert_eq!(
            serde_json::to_value(EnvironmentState::Interrupted).unwrap(),
            "interrupted"
        );
    }
}
