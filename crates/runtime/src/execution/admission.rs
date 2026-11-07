//! A binding change must not race an operation that already selected Host.
use anyhow::{anyhow, Result};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

#[derive(Default)]
pub(crate) struct Admissions(Mutex<HashMap<String, Weak<RwLock<()>>>>);

impl Admissions {
    fn gate(&self, thread: &str) -> Arc<RwLock<()>> {
        let mut gates = self.0.lock().unwrap();
        if gates.len() > 256 {
            gates.retain(|_, gate| gate.strong_count() > 0);
        }
        if let Some(gate) = gates.get(thread).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(RwLock::new(()));
        gates.insert(thread.into(), Arc::downgrade(&gate));
        gate
    }
}

impl crate::Supervisor {
    /// Acquire before choosing the execution context, and retain until the
    /// operation ends. Nested readers are safe: transitions never queue writers.
    pub async fn hold_execution(&self, thread: &str) -> OwnedRwLockReadGuard<()> {
        self.interaction.admission.gate(thread).read_owned().await
    }

    pub fn try_hold_execution(&self, thread: &str) -> Result<OwnedRwLockReadGuard<()>> {
        self.interaction
            .admission
            .gate(thread)
            .try_read_owned()
            .map_err(|_| {
                anyhow!("conflict: thread execution environment is changing; retry shortly")
            })
    }

    /// Refuse a binding switch while an admitted operation can still use its
    /// previous context. This is separate from harness release's process gate.
    pub fn try_change_execution(&self, thread: &str) -> Result<OwnedRwLockWriteGuard<()>> {
        self.interaction
            .admission
            .gate(thread)
            .try_write_owned()
            .map_err(|_| anyhow!("conflict: thread execution is active; retry after it finishes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn binding_switch_cannot_pass_an_admitted_host_operation() {
        let admissions = Admissions::default();
        let gate = admissions.gate("thread");
        let first = gate.clone().read_owned().await;
        let nested = admissions.gate("thread").read_owned().await;
        assert!(gate.clone().try_write_owned().is_err());
        assert!(admissions.gate("other").try_write_owned().is_ok());
        drop(first);
        assert!(gate.clone().try_write_owned().is_err());
        drop(nested);
        let transition = gate.clone().try_write_owned().unwrap();
        assert!(gate.clone().try_read_owned().is_err());
        drop(transition);
        assert!(gate.try_read_owned().is_ok());
    }
}
