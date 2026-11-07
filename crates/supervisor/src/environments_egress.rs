//! Authenticated HTTPS CONNECT only. The sandbox has no routed network; exact
//! host grants are checked before resolving, and resolved private IPs are denied.
use crate::environments::Gateway;
use anyhow::{ensure, Context, Result};
use base64::Engine;
use std::net::IpAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[cfg(unix)]
pub(crate) fn start(g: Gateway) -> Result<()> {
    use std::os::fd::AsRawFd;
    let directory = g
        .state
        .environments
        .bridge_directory(&g.environment_id, g.generation)?;
    let path = directory.join("egress.sock");
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    let parent = std::fs::File::open(directory)?;
    let listener = tokio::net::UnixListener::bind(format!(
        "/proc/self/fd/{}/egress.sock",
        parent.as_raw_fd()
    ))?;
    crate::environments::secure_socket(&path)?;
    let limit = std::sync::Arc::new(tokio::sync::Semaphore::new(16));
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {value=listener.accept()=>value,_=tokio::time::sleep(std::time::Duration::from_secs(1))=>{
                if g.state.environments.validate_generation(&g.environment_id,g.generation).is_err() {break} continue
            }};
            let Ok((socket, _)) = accepted else { break };
            let Ok(permit) = limit.clone().try_acquire_owned() else {
                continue;
            };
            let g = g.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(3600), connect(g, socket))
                        .await;
            });
        }
    });
    Ok(())
}
#[cfg(unix)]
async fn connect(g: Gateway, mut socket: tokio::net::UnixStream) -> Result<()> {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    let header_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !header.ends_with(b"\r\n\r\n") {
        ensure!(header.len() < 16384, "proxy header too large");
        let n = tokio::time::timeout_at(header_deadline, socket.read(&mut byte)).await??;
        ensure!(n > 0, "proxy request closed");
        header.push(byte[0]);
    }
    let text = std::str::from_utf8(&header)?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next().context("missing CONNECT")?.split_whitespace();
    ensure!(
        first.next() == Some("CONNECT"),
        "proxy supports HTTPS CONNECT only"
    );
    let authority = first.next().context("destination required")?;
    ensure!(
        first.next() == Some("HTTP/1.1") && first.next().is_none(),
        "unsupported proxy request"
    );
    let (host, port) = authority
        .rsplit_once(':')
        .context("destination port required")?;
    ensure!(
        port == "443"
            && !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'),
        "destination must be an exact DNS host on port 443"
    );
    let mut token = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("proxy-authorization") {
                let value = value.trim();
                if let Some(value) = value.strip_prefix("Basic ") {
                    let decoded = base64::engine::general_purpose::STANDARD.decode(value)?;
                    token = Some(
                        std::str::from_utf8(&decoded)?
                            .split(':')
                            .next()
                            .unwrap_or("")
                            .to_owned(),
                    );
                }
            }
        }
    }
    let thread = pockymoe_runtime::execution::validate_capability(
        token.as_deref().context("proxy capability required")?,
        &g.environment_id,
        g.generation,
    )?;
    let context = g
        .state
        .execution_context(&thread)?
        .context("unbound proxy thread")?;
    let env = context.checked()?;
    ensure!(
        context.environment_id == g.environment_id && context.generation == g.generation,
        "stale proxy generation"
    );
    ensure!(
        g.state
            .environments
            .egress_hosts(&env.id)?
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(host)),
        "destination not authorized"
    );
    let addresses = tokio::net::lookup_host((host, 443))
        .await?
        .collect::<Vec<_>>();
    ensure!(
        !addresses.is_empty() && addresses.iter().all(|a| public(a.ip())),
        "private or metadata destination denied"
    );
    let mut outbound = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::net::TcpStream::connect(addresses.as_slice()),
    )
    .await??;
    outbound.set_nodelay(true)?;
    socket
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    tokio::select! {
        result=tokio::io::copy_bidirectional(&mut socket,&mut outbound)=>{result?;},
        _=async{loop{tokio::time::sleep(std::time::Duration::from_millis(250)).await;if context.checked().is_err(){break}}}=>{}
    };
    Ok(())
}
fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || b == 0 || b == 2))
                || (a == 198 && (b == 18 || b == 19 || b == 51 && c == 100))
                || (a == 203 && b == 0 && c == 113)
                || (a == 100 && (64..=127).contains(&b)))
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public(IpAddr::V4(v4));
            }
            let first = ip.segments()[0];
            !ip.is_loopback()
                && !ip.is_unspecified()
                && (first & 0xe000) == 0x2000
                && first != 0x2002
                && !(first == 0x2001 && (ip.segments()[1] < 0x0200 || ip.segments()[1] == 0x0db8))
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn egress_denies_internal_metadata_and_mapped_addresses() {
        for ip in [
            "127.0.0.1",
            "169.254.169.254",
            "10.2.3.4",
            "172.16.2.3",
            "192.168.1.1",
            "100.100.100.200",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "2002:7f00:1::1",
        ] {
            assert!(!public(ip.parse().unwrap()), "{ip}");
        }
        assert!(public("1.1.1.1".parse().unwrap()));
        assert!(public("2606:4700:4700::1111".parse().unwrap()));
    }
}
