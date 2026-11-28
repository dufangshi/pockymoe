use crate::acp::rpc::AcpProcess;
use anyhow::{bail, Result};
use serde_json::Value;
use std::collections::VecDeque;
use std::future::Future;
use tokio::sync::mpsc;

pub(super) struct Notifications {
    receiver: mpsc::Receiver<Value>,
    buffered: VecDeque<(Value, usize)>,
    bytes: usize,
}

impl Notifications {
    pub(super) fn new(receiver: mpsc::Receiver<Value>) -> Self {
        Self {
            receiver,
            buffered: VecDeque::new(),
            bytes: 0,
        }
    }

    pub(super) async fn recv(&mut self) -> Option<Value> {
        if let Some((notification, bytes)) = self.buffered.pop_front() {
            self.bytes -= bytes;
            return Some(notification);
        }
        self.receiver.recv().await
    }

    pub(super) async fn request(
        &mut self,
        process: &AcpProcess,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        self.wait(process, method, process.request(method, params))
            .await
    }

    pub(super) async fn wait(
        &mut self,
        process: &AcpProcess,
        method: &str,
        response: impl Future<Output = Result<Value>>,
    ) -> Result<Value> {
        tokio::pin!(response);
        // Responses and notifications arrive on the same pipe. Keep reading
        // while a control request is outstanding and replay the notifications
        // in order when the turn starts consuming them.
        loop {
            tokio::select! {
                biased;
                result = &mut response => return result,
                notification = self.receiver.recv() => {
                    let Some(notification) = notification else {
                        return response.await;
                    };
                    match notification["_remoteMethod"].as_str() {
                        Some("thread/started" | "thread/status/changed" | "thread/settings/updated") => continue,
                        Some("mcpServer/startupStatus/updated") if notification["status"] != "failed" => continue,
                        _ => {}
                    }
                    let bytes = serde_json::to_vec(&notification)?.len();
                    if self.buffered.len() >= 128 || self.bytes + bytes > 8 * 1024 * 1024 {
                        let _ = process.shutdown().await;
                        bail!("Codex notification buffer exceeded while waiting for {method}");
                    }
                    self.bytes += bytes;
                    self.buffered.push_back((notification, bytes));
                }
            }
        }
    }
}
