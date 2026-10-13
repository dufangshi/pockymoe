//! Supervisor restart ownership, independent of any particular process manager.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestartStrategy {
    #[default]
    ExternalUpdater,
    HostManaged,
}

impl std::str::FromStr for RestartStrategy {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "" | "external-updater" => Ok(Self::ExternalUpdater),
            "host-managed" => Ok(Self::HostManaged),
            _ => anyhow::bail!(
                "POCKYMOE_SUPERVISOR_RESTART_STRATEGY must be external-updater or host-managed"
            ),
        }
    }
}

/// The owning process manager must relaunch on this normal exit, using a stable path.
pub const HOST_MANAGED_RESTART_EXIT_CODE: i32 = 0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_strategy_selection_defaults_and_rejects_invalid_configuration() {
        assert_eq!(RestartStrategy::default(), RestartStrategy::ExternalUpdater);
        for value in ["", "external-updater", " external-updater "] {
            assert_eq!(
                value.parse::<RestartStrategy>().unwrap(),
                RestartStrategy::ExternalUpdater
            );
        }
        assert_eq!(
            "host-managed".parse::<RestartStrategy>().unwrap(),
            RestartStrategy::HostManaged
        );
        assert!("host-manged".parse::<RestartStrategy>().is_err());
        assert_eq!(
            serde_json::to_value(RestartStrategy::HostManaged).unwrap(),
            "host-managed"
        );
    }
}
