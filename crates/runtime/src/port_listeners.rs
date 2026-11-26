//! Which processes accept TCP connections on which ports. Used to tell a
//! caller that ending a thread's process tree would stop a web service a
//! preview address points at; never to grant or deny access to a port.
#[cfg(target_os = "linux")]
use std::collections::HashMap;
use std::collections::HashSet;

/// `(port, pid)` for each of `ports` that one of `pids` listens on. A port that
/// several of them hold (forked workers inherit the socket) is reported once,
/// for the first of `pids` holding it. Empty when the platform's source is
/// unreadable: the callers only warn, so not knowing must not block them.
pub(crate) fn among(pids: &[u32], ports: &[u16]) -> Vec<(u16, u32)> {
    if pids.is_empty() || ports.is_empty() {
        return Vec::new();
    }
    let wanted: HashSet<u16> = ports.iter().copied().collect();
    let found = platform::listeners(pids, &wanted);
    let mut seen = HashSet::new();
    let mut out: Vec<(u16, u32)> = pids
        .iter()
        .flat_map(|pid| found.iter().filter(move |(_, owner)| owner == pid).copied())
        .filter(|(port, _)| seen.insert(*port))
        .collect();
    out.sort_unstable();
    out
}

/// Listening sockets from `/proc/net/tcp{,6}` lines: `(port, inode)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn proc_net_listeners(table: &str, wanted: &HashSet<u16>) -> Vec<(u16, u64)> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            // sl local remote st tx:rx tr:when retrnsmt uid timeout inode ...
            if fields.len() < 10 || fields[3] != "0A" {
                return None;
            }
            let port = u16::from_str_radix(fields[1].rsplit_once(':')?.1, 16).ok()?;
            let inode = fields[9].parse().ok()?;
            wanted.contains(&port).then_some((port, inode))
        })
        .collect()
}

/// `lsof -nP -iTCP -sTCP:LISTEN -Fpn`: a `p<pid>` line, then one `n<addr>:<port>`
/// line per listening socket of that process.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn lsof_listeners(output: &str, wanted: &HashSet<u16>) -> Vec<(u16, u32)> {
    let mut pid = None;
    let mut out = Vec::new();
    for line in output.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse::<u32>().ok();
        } else if let (Some(name), Some(pid)) = (line.strip_prefix('n'), pid) {
            let port = name
                .rsplit_once(':')
                .and_then(|(_, port)| port.parse().ok());
            if let Some(port) = port.filter(|port| wanted.contains(port)) {
                out.push((port, pid));
            }
        }
    }
    out
}

/// `netstat -ano -p TCP`: `TCP  <local>:<port>  <remote>  LISTENING  <pid>`.
#[cfg_attr(not(windows), allow(dead_code))]
fn netstat_listeners(output: &str, wanted: &HashSet<u16>) -> Vec<(u16, u32)> {
    output
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 5 || fields[0] != "TCP" || fields[3] != "LISTENING" {
                return None;
            }
            let port = fields[1].rsplit_once(':')?.1.parse().ok()?;
            let pid = fields[4].parse().ok()?;
            wanted.contains(&port).then_some((port, pid))
        })
        .collect()
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    pub(super) fn listeners(pids: &[u32], wanted: &HashSet<u16>) -> Vec<(u16, u32)> {
        let mut inodes: HashMap<u64, u16> = HashMap::new();
        for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
            if let Ok(text) = std::fs::read_to_string(table) {
                for (port, inode) in proc_net_listeners(&text, wanted) {
                    inodes.insert(inode, port);
                }
            }
        }
        if inodes.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for pid in pids {
            let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(target) = std::fs::read_link(entry.path()) else {
                    continue;
                };
                let inode = target
                    .to_str()
                    .and_then(|name| name.strip_prefix("socket:["))
                    .and_then(|name| name.strip_suffix(']'))
                    .and_then(|inode| inode.parse::<u64>().ok());
                if let Some(port) = inode.and_then(|inode| inodes.get(&inode)) {
                    out.push((*port, *pid));
                }
            }
        }
        out
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::*;

    fn run(program: &str, args: &[&str]) -> Option<String> {
        let mut command = std::process::Command::new(program);
        command.args(args);
        crate::child_process::hide_std(&mut command);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let (_, bytes, _) =
            crate::child_process::run_bounded(&mut command, 4 << 20, deadline).ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub(super) fn listeners(pids: &[u32], wanted: &HashSet<u16>) -> Vec<(u16, u32)> {
        let found = if cfg!(windows) {
            run("netstat", &["-ano", "-p", "TCP"]).map(|text| netstat_listeners(&text, wanted))
        } else {
            run("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpn"])
                .map(|text| lsof_listeners(&text, wanted))
        };
        let pids: HashSet<u32> = pids.iter().copied().collect();
        found
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, pid)| pids.contains(pid))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted(ports: &[u16]) -> HashSet<u16> {
        ports.iter().copied().collect()
    }

    #[test]
    fn proc_net_keeps_only_listening_sockets_on_wanted_ports() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:10D6 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4711 1 0000000000000000 100 0 0 10 0\n\
   1: 0100007F:10D6 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 4712 1 0000000000000000 20 4 30 10 -1\n\
   2: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 99 1 0000000000000000 100 0 0 10 0\n";
        assert_eq!(
            proc_net_listeners(table, &wanted(&[4310])),
            vec![(4310, 4711)]
        );
        assert!(proc_net_listeners(table, &wanted(&[5000])).is_empty());
        // The IPv6 table has the same columns with a longer address.
        let v6 = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 00000000000000000000000001000000:10D6 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4800 1 0000000000000000 100 0 0 10 0\n";
        assert_eq!(proc_net_listeners(v6, &wanted(&[4310])), vec![(4310, 4800)]);
    }

    #[test]
    fn lsof_and_netstat_output_name_the_listening_process() {
        let lsof = "p812\nn127.0.0.1:4310\nn[::1]:4310\np900\nn*:8080\n";
        assert_eq!(
            lsof_listeners(lsof, &wanted(&[4310, 8080])),
            vec![(4310, 812), (4310, 812), (8080, 900)]
        );
        let netstat = "\r\nActive Connections\r\n\r\n  Proto  Local Address          Foreign Address        State           PID\r\n  TCP    127.0.0.1:4310         0.0.0.0:0              LISTENING       5120\r\n  TCP    127.0.0.1:4310         127.0.0.1:51000        ESTABLISHED     5120\r\n  TCP    [::]:8080              [::]:0                 LISTENING       640\r\n";
        assert_eq!(
            netstat_listeners(netstat, &wanted(&[4310, 8080])),
            vec![(4310, 5120), (8080, 640)]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_listener_of_this_process_is_found_by_its_port_and_no_other() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let me = std::process::id();
        assert_eq!(among(&[me], &[port]), vec![(port, me)]);
        // Reported once even when the same process is listed twice.
        assert_eq!(among(&[me, me], &[port]), vec![(port, me)]);
        assert!(
            among(&[1], &[port]).is_empty(),
            "another process does not hold it"
        );
        drop(listener);
        assert!(
            among(&[me], &[port]).is_empty(),
            "a closed listener is gone"
        );
    }
}
