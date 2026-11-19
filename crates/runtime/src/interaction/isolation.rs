//! Agent isolation: a device or workspace that agents elsewhere cannot list, read
//! or message through the CLI, other devices or Treer. The owner's own access and
//! sharing are unaffected. Contract: docs/agent-isolation.md.
use crate::Supervisor;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const KEY: &str = "agent:isolation";
const DEVICE_ISOLATED: &str = "forbidden: this device is isolated from agents on other devices";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIsolation {
    /// Agents on other devices cannot reach this device.
    #[serde(default)]
    pub device: bool,
    /// Agents outside these workspaces cannot reach them.
    #[serde(default)]
    pub workspaces: BTreeSet<String>,
}

/// Who is asking, as far as the Supervisor can verify it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentRequester<'a> {
    /// The computer's owner: the Web UI or the machine's own CLI credential.
    Owner,
    /// A thread on this device, identified by its own credential.
    Thread(&'a str),
    /// Anything outside every workspace of this device that is not the owner:
    /// another device, or a Treer agent that is not one of this hub's threads.
    Outsider,
    /// A thread or command on another device.
    Peer,
}

fn read_isolation(conn: &Connection) -> Result<AgentIsolation> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM kv WHERE key=?1", [KEY], |row| row.get(0))
        .optional()?;
    match raw {
        Some(raw) => serde_json::from_str(&raw).context("Invalid stored agent isolation policy"),
        None => Ok(AgentIsolation::default()),
    }
}

impl Supervisor {
    pub fn agent_isolation(&self) -> Result<AgentIsolation> {
        self.db.with(read_isolation)
    }

    fn update_isolation(&self, change: impl FnOnce(&mut AgentIsolation)) -> Result<AgentIsolation> {
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            let mut policy = read_isolation(&tx)?;
            change(&mut policy);
            tx.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![KEY, serde_json::to_string(&policy)?])?;
            tx.commit()?;
            Ok(policy)
        })
    }

    pub fn set_device_isolation(&self, isolated: bool) -> Result<AgentIsolation> {
        self.update_isolation(|policy| policy.device = isolated)
    }

    pub fn set_workspace_isolation(
        &self,
        workspace_id: &str,
        isolated: bool,
    ) -> Result<AgentIsolation> {
        self.get_workspace(workspace_id)?;
        self.update_isolation(|isolation| {
            if isolated {
                isolation.workspaces.insert(workspace_id.to_owned());
            } else {
                isolation.workspaces.remove(workspace_id);
            }
        })
    }

    /// Refuses requests from other devices while this device is isolated.
    pub fn ensure_peer_reaches_device(&self) -> Result<()> {
        if self.agent_isolation()?.device {
            bail!("{DEVICE_ISOLATED}");
        }
        Ok(())
    }

    /// Whether this requester may see the workspace and anything in it.
    pub fn agent_reaches_workspace(&self, requester: AgentRequester, workspace_id: &str) -> bool {
        if requester == AgentRequester::Owner {
            return true;
        }
        let Ok(isolation) = self.agent_isolation() else {
            return false;
        };
        if requester == AgentRequester::Peer && isolation.device {
            return false;
        }
        if !isolation.workspaces.contains(workspace_id) {
            return true;
        }
        match requester {
            AgentRequester::Thread(caller) => self
                .get_thread(caller)
                .is_ok_and(|thread| thread.workspace_id == workspace_id),
            _ => false,
        }
    }

    /// Refuses a requester that an isolated device or workspace keeps out.
    pub fn ensure_agent_reaches_workspace(
        &self,
        requester: AgentRequester,
        workspace_id: &str,
    ) -> Result<()> {
        if self.agent_reaches_workspace(requester, workspace_id) {
            return Ok(());
        }
        if requester == AgentRequester::Peer {
            self.ensure_peer_reaches_device()?;
        }
        let label = self
            .get_workspace(workspace_id)
            .map(|workspace| workspace.label)
            .unwrap_or_else(|_| workspace_id.to_owned());
        bail!("forbidden: workspace {label} is isolated from agents outside it")
    }

    /// Like `ensure_agent_reaches_workspace` for the thread's workspace. An
    /// unknown thread passes, so the operation reports it as not found.
    pub fn ensure_agent_reaches_thread(
        &self,
        requester: AgentRequester,
        thread_id: &str,
    ) -> Result<()> {
        if requester == AgentRequester::Owner {
            return Ok(());
        }
        let workspace: Option<String> = self.db.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT workspace_id FROM threads WHERE id=?1",
                    [thread_id],
                    |r| r.get(0),
                )
                .optional()?)
        })?;
        match workspace {
            Some(id) => self.ensure_agent_reaches_workspace(requester, &id),
            None => Ok(()),
        }
    }
}
