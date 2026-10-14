//! Startup policy for subsystems owned by an external hosting control plane.
use serde::Serialize;
use std::{fmt, str::FromStr};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HostMode {
    #[default]
    Standalone,
    Hosted,
}

impl FromStr for HostMode {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "standalone" => Ok(Self::Standalone),
            "hosted" => Ok(Self::Hosted),
            _ => anyhow::bail!("Invalid host mode; expected standalone or hosted"),
        }
    }
}
impl fmt::Display for HostMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Standalone => "standalone",
            Self::Hosted => "hosted",
        })
    }
}
impl HostMode {
    pub fn from_env() -> anyhow::Result<Self> {
        match std::env::var("POCKYMOE_HOST_MODE") {
            Ok(value) => value.parse(),
            Err(std::env::VarError::NotPresent) => Ok(Self::default()),
            Err(_) => anyhow::bail!("Invalid POCKYMOE_HOST_MODE"),
        }
    }

    pub const fn capabilities(self) -> HostCapabilities {
        let enabled = matches!(self, Self::Standalone);
        HostCapabilities {
            relay: enabled,
            account_binding: enabled,
            device_peer: enabled,
            previews: enabled,
        }
    }

    pub fn restart_strategy(
        self,
        explicit: Option<&str>,
    ) -> anyhow::Result<crate::RestartStrategy> {
        match explicit {
            Some(value) => value.parse(),
            None if self == Self::Hosted => Ok(crate::RestartStrategy::HostManaged),
            None => Ok(crate::RestartStrategy::ExternalUpdater),
        }
    }

    pub fn treer_enabled(self, hosted_by: Option<&str>) -> anyhow::Result<bool> {
        match hosted_by {
            None | Some("") => Ok(false),
            Some("treer") if self == Self::Hosted => Ok(true),
            Some("treer") => {
                anyhow::bail!("POCKYMOE_HOSTED_BY=treer requires POCKYMOE_HOST_MODE=hosted")
            }
            Some(_) => anyhow::bail!("Invalid POCKYMOE_HOSTED_BY; expected treer"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCapabilities {
    pub relay: bool,
    pub account_binding: bool,
    pub device_peer: bool,
    pub previews: bool,
}

pub const FEATURE_DISABLED_CODE: &str = "feature_disabled_by_host_mode";

#[derive(Debug, thiserror::Error)]
#[error("feature_disabled_by_host_mode: {0} is disabled by the startup host policy")]
pub struct FeatureDisabled(pub &'static str);

pub fn require_feature(enabled: bool, feature: &'static str) -> anyhow::Result<()> {
    if !enabled {
        return Err(FeatureDisabled(feature).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_and_enablement() {
        assert_eq!(
            HostMode::Hosted.restart_strategy(None).unwrap(),
            crate::RestartStrategy::HostManaged
        );
        assert_eq!(
            HostMode::Hosted
                .restart_strategy(Some("external-updater"))
                .unwrap(),
            crate::RestartStrategy::ExternalUpdater
        );
        assert_eq!(
            HostMode::Standalone.restart_strategy(None).unwrap(),
            crate::RestartStrategy::ExternalUpdater
        );
        assert!(HostMode::Hosted.restart_strategy(Some("invalid")).is_err());
        assert!(HostMode::Standalone.treer_enabled(Some("treer")).is_err());
        assert!(HostMode::Hosted.treer_enabled(Some("treer")).unwrap());
        assert!(!HostMode::Hosted.treer_enabled(None).unwrap());
    }
}
