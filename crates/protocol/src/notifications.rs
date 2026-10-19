//! One policy for account notifications, shared by Relay and local owner delivery.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NotificationScope {
    #[default]
    Subscribed,
    All,
    Selected,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationDevice {
    pub device_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationWorkspace {
    pub device_id: String,
    pub workspace_id: String,
    pub name: String,
    pub device_name: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct NotificationSettings {
    pub scope: NotificationScope,
    pub devices: Vec<NotificationDevice>,
    pub workspaces: Vec<NotificationWorkspace>,
}

impl NotificationSettings {
    pub fn valid(&self) -> bool {
        let text = |v: &str| !v.is_empty() && v.len() <= 1024;
        self.devices.len() + self.workspaces.len() <= 200
            && self
                .devices
                .iter()
                .all(|d| text(&d.device_id) && text(&d.name))
            && self.workspaces.iter().all(|w| {
                text(&w.device_id) && text(&w.workspace_id) && text(&w.name) && text(&w.device_name)
            })
    }
}

/// Missing workspace metadata from an old Supervisor cannot match a workspace
/// allowlist. Device selection and exact turn subscriptions still work.
pub fn should_notify_turn(
    settings: &NotificationSettings,
    device: &str,
    workspace: Option<&str>,
    subscribed: bool,
    status: &str,
) -> bool {
    matches!(status, "completed" | "failed")
        && match settings.scope {
            NotificationScope::Subscribed => subscribed,
            NotificationScope::All => true,
            NotificationScope::Selected => {
                settings.devices.iter().any(|d| d.device_id == device)
                    || settings.workspaces.iter().any(|w| {
                        w.device_id == device && Some(w.workspace_id.as_str()) == workspace
                    })
            }
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_scope_and_terminal_status_share_one_policy() {
        let mut settings = NotificationSettings::default();
        assert!(!should_notify_turn(
            &settings,
            "a",
            Some("w"),
            false,
            "completed"
        ));
        assert!(should_notify_turn(
            &settings,
            "a",
            Some("w"),
            true,
            "failed"
        ));
        settings.scope = NotificationScope::All;
        assert!(should_notify_turn(&settings, "a", None, false, "completed"));
        settings.scope = NotificationScope::Selected;
        settings.devices.push(NotificationDevice {
            device_id: "a".into(),
            name: "A".into(),
        });
        settings.workspaces.push(NotificationWorkspace {
            device_id: "b".into(),
            workspace_id: "w".into(),
            name: "W".into(),
            device_name: "B".into(),
        });
        assert!(should_notify_turn(&settings, "a", None, false, "failed"));
        assert!(should_notify_turn(
            &settings,
            "b",
            Some("w"),
            false,
            "completed"
        ));
        assert!(!should_notify_turn(
            &settings,
            "c",
            Some("w"),
            true,
            "completed"
        ));
        assert!(!should_notify_turn(&settings, "b", None, true, "completed"));
        for scope in [
            NotificationScope::Subscribed,
            NotificationScope::All,
            NotificationScope::Selected,
        ] {
            settings.scope = scope;
            for status in ["interrupted", "recovering", "inProgress"] {
                assert!(!should_notify_turn(&settings, "a", Some("w"), true, status));
            }
        }
        assert_eq!(
            serde_json::from_str::<NotificationSettings>("{}").unwrap(),
            NotificationSettings::default()
        );
    }
}
