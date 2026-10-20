//! A slow peer must reconnect, never retain an unlimited backlog or silently lose events.
//! The bound is loose: a burst of streamed output or a large response must not
//! tear a healthy connection down (a 64-message bound did, several times an hour).
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{mpsc, Notify};
#[derive(Clone)]
pub(crate) struct Sender<T> {
    tx: mpsc::Sender<T>,
    failed: Arc<AtomicBool>,
    wake: Arc<Notify>,
}
/// Messages a connection may have waiting before it is considered stuck.
pub(crate) const CAPACITY: usize = 8192;
pub(crate) struct Receiver<T> {
    rx: mpsc::Receiver<T>,
    failed: Arc<AtomicBool>,
    wake: Arc<Notify>,
}
pub(crate) fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = mpsc::channel(CAPACITY);
    let failed = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(Notify::new());
    (
        Sender {
            tx,
            failed: failed.clone(),
            wake: wake.clone(),
        },
        Receiver { rx, failed, wake },
    )
}
impl<T> Sender<T> {
    pub fn send(&self, value: T) -> Result<(), ()> {
        if self.failed.load(Ordering::Acquire) {
            return Err(());
        }
        self.tx.try_send(value).map_err(|_| {
            self.failed.store(true, Ordering::Release);
            self.wake.notify_one();
        })
    }
}
impl<T> Receiver<T> {
    /// Whether the channel closed because its bound was reached.
    pub fn overflowed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    /// A message already waiting, without blocking; none after an overflow.
    pub fn try_recv(&mut self) -> Option<T> {
        if self.failed.load(Ordering::Acquire) {
            return None;
        }
        self.rx.try_recv().ok()
    }
    pub async fn recv(&mut self) -> Option<T> {
        if self.failed.load(Ordering::Acquire) {
            return None;
        }
        tokio::select! {biased; _=self.wake.notified()=>None, value=self.rx.recv()=>value}
    }
}
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn overflow_disconnects_instead_of_dropping_a_message() {
        let (sender, mut receiver) = super::channel();
        for n in 0..super::CAPACITY {
            sender.send(n).unwrap();
        }
        assert!(sender.send(super::CAPACITY).is_err());
        assert!(receiver.overflowed());
        assert_eq!(receiver.recv().await, None);
    }
}
