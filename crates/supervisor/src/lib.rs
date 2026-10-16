mod auth;
mod automations;
mod bounded_channel;
pub mod distribution;
mod export;
mod file_documents;
mod grants;
mod host_policy;
mod http;
mod interaction;
mod isolation;
#[cfg(test)]
mod isolation_tests;
mod linked_files;
#[cfg(test)]
mod local_passcode_tests;
mod management;
mod owner_trust;
mod preview_cli;
// Device-to-device messaging and files (docs/cross-device-peer.zh.md).
mod peer_api;
mod peer_files;
mod peer_link;
mod peer_send;
mod plugins;
mod ports;
mod secure_transport;
mod shells;
mod socket;
mod treer;
mod tunnel;
mod upstreams;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use pockymoe_runtime::Supervisor;
use tokio::net::TcpListener;

pub use http::router;
pub use owner_trust::{
    bind_account, binding_status, reset_account_trust, validate_binding, validate_web_origin,
};
pub use tunnel::run_relay_tunnel;

pub async fn serve(state: Arc<Supervisor>) -> Result<()> {
    auth::validate_config(&state.config)?;
    state.spawn_live_item_persister();
    state.spawn_execution_observer();
    state.spawn_harness_release();
    let addr: SocketAddr = format!("{}:{}", state.config.host, state.config.port).parse()?;
    let listener = TcpListener::bind(addr).await?;
    let cli = state.configure_cli(format!(
        "http://127.0.0.1:{}",
        listener.local_addr()?.port()
    ));
    let path = state.config.database_url.with_extension("cli.json");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options.open(&path)?.write_all(
        serde_json::to_string(&serde_json::json!({"url":cli.url,"token":cli.token,"hostMode":state.config.host_mode,"hostCapabilities":state.host_capabilities}))?.as_bytes(),
    )?;
    tracing::info!("supervisor listening on {addr}");
    if state.host_capabilities.relay && state.config.mode == pockymoe_protocol::Mode::Relay {
        let tunnel_state = state.clone();
        tokio::spawn(async move {
            if let Err(err) = run_relay_tunnel(tunnel_state).await {
                tracing::error!(error = %err, "relay tunnel exited");
            }
        });
    }
    let hub = treer::Hub::new(state.clone(), listener.local_addr()?.port())?;
    // A restart with no active turns still needs its startup outcome recorded.
    if state.defer_update_recovery() || distribution::host_managed_pending(&state) {
        tokio::spawn(management::recover_after_update(state.clone()));
    } else {
        state.spawn_inbox_recovery();
    }
    axum::serve(
        listener,
        http::router_with_treer(state, hub).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// Load or create the identity's public fingerprint without booting a runtime or listener.
pub fn relay_device_fingerprint(database: &std::path::Path) -> anyhow::Result<String> {
    secure_transport::fingerprint(database)
}
