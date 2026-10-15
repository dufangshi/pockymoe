//! Optional host integration at thread lifecycle boundaries. Standalone installs
//! leave this unset; external identities and policy stay in the host crate.
use anyhow::Result;
use pockymoe_protocol::ThreadDto;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadOrigin {
    Local,
    Delegate {
        caller_thread_id: String,
    },
    Browser {
        caller_thread_id: String,
    },
    Fork {
        source_thread_id: String,
    },
    /// A host workload is already running and is creating its first thread.
    Attached {
        external_id: String,
    },
    Hidden,
}

#[async_trait::async_trait]
pub trait ThreadHost: Send + Sync {
    async fn created(&self, thread: &ThreadDto, origin: &ThreadOrigin) -> Result<()>;
    fn before_delete(&self, thread: &ThreadDto) -> Result<()>;
    fn before_close(&self, thread: &ThreadDto) -> Result<()>;
    async fn before_prompt(&self, thread: &ThreadDto) -> Result<()>;
    fn before_rename(&self, thread: &ThreadDto, title: &str) -> Result<()>;
    fn reserve_closed_names(&self) -> bool {
        false
    }
    fn visible(&self, _id: &str) -> bool {
        true
    }
}
