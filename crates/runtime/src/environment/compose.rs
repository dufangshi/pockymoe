use anyhow::{ensure, Result};
use pockymoe_protocol::{EnvironmentDto, EnvironmentMountDto};
use serde_json::{json, Value};

pub(super) const WORKER_PATH: &str = "/opt/pockymoe/bin/pockymoe";
pub(super) const STATE_PATH: &str = "/pockymoe/state";
pub(super) const BRIDGE_PATH: &str = "/pockymoe/bridge";

fn literal(value: &str) -> String {
    value.replace('$', "$$")
}

pub(super) fn render(dto: &EnvironmentDto, image: &str, uid_gid: &str) -> Result<String> {
    ensure!(
        !uid_gid.starts_with("0:"),
        "environment processes cannot run as root"
    );
    let mounts: Vec<Value> = dto.mounts.iter().map(|mount: &EnvironmentMountDto| json!({
        "type":"bind", "source":literal(&mount.source), "target":literal(&mount.target),
        "read_only":mount.read_only, "bind":{"propagation":"rprivate","create_host_path":false}
    })).collect();
    let working = dto.worktree_path.as_ref().unwrap_or(&dto.workspace_path);
    let service = json!({
        "image":literal(image), "container_name":format!("pockymoe-env-{}-g{}",dto.id,dto.generation),
        "entrypoint":[WORKER_PATH], "command":["environment-worker","serve"],
        "user":uid_gid, "working_dir":literal(working), "init":true,
        "network_mode":"none", "ipc":"private", "privileged":false,
        "cap_drop":["ALL"], "security_opt":["no-new-privileges:true"],
        "read_only":true, "restart":"no", "stop_grace_period":"10s",
        "cpus":dto.limits.cpu, "mem_limit":dto.limits.memory_bytes,
        "memswap_limit":dto.limits.memory_bytes, "pids_limit":dto.limits.pids,
        "ulimits":{"nofile":{"soft":dto.limits.nofile,"hard":dto.limits.nofile}},
        "tmpfs":["/tmp:rw,nosuid,nodev,size=268435456","/run:rw,nosuid,nodev,size=16777216"],
        "environment":{
            "HOME":STATE_PATH, "CODEX_HOME":"/pockymoe/state/codex",
            "XDG_CONFIG_HOME":"/pockymoe/state/config", "XDG_CACHE_HOME":"/pockymoe/state/cache",
            "POCKYMOE_ENVIRONMENT_WORKER":"1",
            "POCKYMOE_ENVIRONMENT_ID":dto.id,"POCKYMOE_ENVIRONMENT_GENERATION":dto.generation.to_string()
        },
        "volumes":mounts,
        "labels":{
            "ca.pockymoe.environment":dto.id,"ca.pockymoe.generation":dto.generation.to_string()
        }
    });
    Ok(serde_yaml::to_string(
        &json!({"services":{"worker":service}}),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pockymoe_protocol::EnvironmentLimitsDto;

    #[test]
    fn compose_is_structured_offline_and_does_not_interpolate_paths() {
        let dto = EnvironmentDto {
            id: "test".into(),
            name: "test".into(),
            connection_id: "c".into(),
            provider: "dokploy".into(),
            thread_id: "t".into(),
            generation: 1,
            state: "stopped".into(),
            container_id: None,
            compose_id: None,
            workspace_path: "/work/$HOME:hello".into(),
            worktree_path: None,
            mounts: vec![EnvironmentMountDto {
                source: "/data/$HOME:hello".into(),
                target: "/work/$HOME:hello".into(),
                read_only: false,
            }],
            limits: EnvironmentLimitsDto {
                cpu: 1.0,
                memory_bytes: 536870912,
                pids: 128,
                nofile: 1024,
            },
            observed_limits: None,
            last_error: None,
            disk_quota_supported: false,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let yaml = render(&dto, "fixture:1", "1000:1000").unwrap();
        let value: Value = serde_yaml::from_str(&yaml).unwrap();
        let worker = &value["services"]["worker"];
        assert_eq!(worker["network_mode"], "none");
        assert_eq!(worker["volumes"][0]["source"], "/data/$$HOME:hello");
        assert_eq!(worker["volumes"][0]["bind"]["create_host_path"], false);
        assert_eq!(worker["read_only"], true);
        assert!(worker.get("ports").is_none());
        assert!(render(&dto, "fixture:1", "0:0").is_err());
    }
}
