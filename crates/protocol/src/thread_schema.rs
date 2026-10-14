//! String discriminants for the thread wire contract. Payloads stay extensible.
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// One mapping drives enum conversion, serialization and the contract inventory.
macro_rules! string_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant,)+
            /// An unrecognized wire value, preserved verbatim for newer peers.
            Unknown(String),
        }

        impl $name {
            pub const KNOWN_VALUES: &'static [&'static str] = &[$($wire,)+];

            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $wire,)+
                    Self::Unknown(value) => value,
                }
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                match value {
                    $($wire => Self::$variant,)+
                    _ => Self::Unknown(value.to_owned()),
                }
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                match value.as_str() {
                    $($wire => Self::$variant,)+
                    _ => Self::Unknown(value),
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                String::deserialize(deserializer).map(Self::from)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        // Keep existing string predicates usable during mechanical migration.
        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool { self.as_str() == *other }
        }
    };
}

string_enum! {
    ThreadItemKind {
        UserMessage => "userMessage",
        AgentMessage => "agentMessage",
        Artifact => "artifact",
        Image => "image",
        ImageView => "imageView",
        Plan => "plan",
        ContextCompaction => "contextCompaction",
        Reasoning => "reasoning",
        CommandExecution => "commandExecution",
        WebSearch => "webSearch",
        FileRead => "fileRead",
        FileChange => "fileChange",
        Hook => "hook",
        AgentToolCall => "agentToolCall",
        SkillToolCall => "skillToolCall",
        ToolCall => "toolCall",
        Generic => "generic",
        Other => "other",
    }
}

string_enum! {
    ThreadStatus {
        Recovering => "recovering",
        Idle => "idle",
        Running => "running",
        Interrupted => "interrupted",
        Failed => "failed",
        NotLoaded => "not_loaded",
        SystemError => "system_error",
    }
}

string_enum! {
    TurnStatus {
        Completed => "completed",
        Interrupted => "interrupted",
        Failed => "failed",
        InProgress => "inProgress",
        Recovering => "recovering",
    }
}

string_enum! {
    ThreadEventType {
        ThreadUpdated => "thread.updated",
        ThreadContextUpdated => "thread.context.updated",
        ThreadSubagentsUpdated => "thread.subagents.updated",
        ThreadGoalUpdated => "thread.goal.updated",
        ThreadGoalCleared => "thread.goal.cleared",
        ThreadTurnTokenUpdated => "thread.turn.token.updated",
        ThreadTurnStarted => "thread.turn.started",
        ThreadItemStarted => "thread.item.started",
        ThreadItemCompleted => "thread.item.completed",
        ThreadPlanUpdated => "thread.plan.updated",
        ThreadRequestCreated => "thread.request.created",
        ThreadRequestResolved => "thread.request.resolved",
        ThreadOutputDelta => "thread.output.delta",
        ThreadTurnCompleted => "thread.turn.completed",
        ThreadPersistenceFailed => "thread.persistence.failed",
        ThreadTurnFailed => "thread.turn.failed",
        ThreadHarnessReady => "thread.harness.ready",
        ThreadCollaborationUpdated => "thread.collaboration.updated",
        ThreadContextDelivered => "thread.context.delivered",
        RuntimeUsageUpdated => "runtime.usage.updated",
        DevicePortsChanged => "device.ports.changed",
    }
}
