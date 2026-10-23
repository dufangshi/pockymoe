//! Health of the relay's host for the admin console's Server section
//! (docs/relay-admin.md "Server"): the relay process, the machine, the cgroup
//! the service runs in and the filesystem that holds the data directory.
//!
//! On Linux every figure comes from small text files under `/proc` and
//! `/sys/fs/cgroup`, read once a minute by the metrics sampler; the disk is
//! measured with `statvfs` on any Unix. A figure the system does not offer is
//! `None` and the console says "not available" instead of failing. Host
//! figures only: nothing about users or their content.
use std::path::Path;
use std::sync::Mutex as StdMutex;
use std::time::Instant;

/// One reading. Rates (CPU, network) need the previous reading and are `None`
/// on the first.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct HostSample {
    pub(crate) pid: u32,
    /// Resident memory of the relay process.
    pub(crate) rss_bytes: Option<u64>,
    /// Virtual memory of the relay process.
    pub(crate) vm_bytes: Option<u64>,
    pub(crate) threads: Option<u64>,
    pub(crate) open_files: Option<u64>,
    pub(crate) open_files_limit: Option<u64>,
    /// CPU time of the process per second of wall time: 1.0 is one core busy.
    pub(crate) process_cpu_cores: Option<f64>,
    /// Logical CPUs of the machine.
    pub(crate) cores: Option<u64>,
    /// Busy share of all cores since the previous reading (0–1).
    pub(crate) cpu: Option<f64>,
    /// Busy share of each core since the previous reading.
    pub(crate) per_core: Vec<f64>,
    pub(crate) load: Option<[f64; 3]>,
    pub(crate) memory_total: Option<u64>,
    pub(crate) memory_available: Option<u64>,
    pub(crate) swap_total: Option<u64>,
    pub(crate) swap_free: Option<u64>,
    /// Bytes received and sent on all interfaces but loopback since the
    /// previous reading, per second.
    pub(crate) net_rx_per_second: Option<f64>,
    pub(crate) net_tx_per_second: Option<f64>,
    /// The cgroup the relay runs in (systemd service, container).
    pub(crate) cgroup: Option<String>,
    pub(crate) cgroup_memory: Option<u64>,
    /// The memory limit of the cgroup (the smallest along its ancestors).
    pub(crate) cgroup_memory_limit: Option<u64>,
    /// The CPU quota of the cgroup in cores.
    pub(crate) cgroup_cpu_limit: Option<f64>,
    /// The filesystem holding the data directory.
    pub(crate) disk_total: Option<u64>,
    pub(crate) disk_used: Option<u64>,
    pub(crate) disk_available: Option<u64>,
}

impl HostSample {
    pub(crate) fn memory_used(&self) -> Option<u64> {
        Some(self.memory_total?.saturating_sub(self.memory_available?))
    }
    pub(crate) fn swap_used(&self) -> Option<u64> {
        Some(self.swap_total?.saturating_sub(self.swap_free?))
    }
    /// The cores the relay may use: the cgroup quota, or every core.
    pub(crate) fn cpu_capacity(&self) -> Option<f64> {
        match (self.cgroup_cpu_limit, self.cores) {
            (Some(limit), Some(cores)) => Some(limit.min(cores as f64)),
            (Some(limit), None) => Some(limit),
            (None, cores) => cores.map(|cores| cores as f64),
        }
    }
}

/// Counters kept between readings.
struct Previous {
    at: Instant,
    process_ticks: Option<u64>,
    cpu: Vec<(u64, u64)>,
    net: Option<(u64, u64)>,
}

/// Reads the host; keeps the previous reading for rates.
#[derive(Default)]
pub(crate) struct HostProbe {
    previous: StdMutex<Option<Previous>>,
    latest: StdMutex<Option<(i64, HostSample)>>,
}

impl HostProbe {
    /// Takes a reading (the sampler, once a minute) and keeps it as the latest.
    pub(crate) fn sample(&self, data_dir: Option<&Path>, at_ms: i64) -> HostSample {
        let now = Instant::now();
        let mut sample = HostSample {
            pid: std::process::id(),
            ..HostSample::default()
        };
        if let Some(dir) = data_dir {
            disk(dir, &mut sample);
        }
        let current = linux::read(&mut sample);
        let mut previous = self.previous.lock().unwrap_or_else(|e| e.into_inner());
        if let (Some(before), Some(current)) = (previous.as_ref(), current.as_ref()) {
            let seconds = now.duration_since(before.at).as_secs_f64();
            if seconds > 0.5 {
                if let (Some(a), Some(b)) = (before.process_ticks, current.process_ticks) {
                    sample.process_cpu_cores =
                        Some(b.saturating_sub(a) as f64 / linux::ticks_per_second() / seconds);
                }
                let busy = |(busy_a, total_a): (u64, u64), (busy_b, total_b): (u64, u64)| {
                    let total = total_b.saturating_sub(total_a);
                    (total > 0).then(|| busy_b.saturating_sub(busy_a) as f64 / total as f64)
                };
                if let (Some(a), Some(b)) = (before.cpu.first(), current.cpu.first()) {
                    sample.cpu = busy(*a, *b).map(|value| value.clamp(0.0, 1.0));
                }
                sample.per_core = before
                    .cpu
                    .iter()
                    .skip(1)
                    .zip(current.cpu.iter().skip(1))
                    .filter_map(|(a, b)| busy(*a, *b))
                    .map(|value| value.clamp(0.0, 1.0))
                    .collect();
                if let (Some((rx_a, tx_a)), Some((rx_b, tx_b))) = (before.net, current.net) {
                    sample.net_rx_per_second = Some(rx_b.saturating_sub(rx_a) as f64 / seconds);
                    sample.net_tx_per_second = Some(tx_b.saturating_sub(tx_a) as f64 / seconds);
                }
            }
        }
        *previous = current.map(|counters| Previous {
            at: now,
            ..counters
        });
        *self.latest.lock().unwrap_or_else(|e| e.into_inner()) = Some((at_ms, sample.clone()));
        sample
    }

    /// The latest reading and when it was taken, if any.
    pub(crate) fn latest(&self) -> Option<(i64, HostSample)> {
        self.latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// Size, use and free space of the filesystem holding `dir`, as `df` counts
/// them: used is what is taken, available what an unprivileged writer may
/// still use.
#[cfg(unix)]
fn disk(dir: &Path, sample: &mut HostSample) {
    if let Ok(stat) = rustix::fs::statvfs(dir) {
        let unit = if stat.f_frsize > 0 {
            stat.f_frsize
        } else {
            stat.f_bsize
        };
        sample.disk_total = Some(stat.f_blocks.saturating_mul(unit));
        sample.disk_used = Some(
            stat.f_blocks
                .saturating_sub(stat.f_bfree)
                .saturating_mul(unit),
        );
        sample.disk_available = Some(stat.f_bavail.saturating_mul(unit));
    }
}

#[cfg(not(unix))]
fn disk(_dir: &Path, _sample: &mut HostSample) {}

/// Readers of `/proc` and `/sys`; on other systems they read nothing.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod linux {
    use super::*;

    pub(super) struct Counters {
        pub(super) process_ticks: Option<u64>,
        pub(super) cpu: Vec<(u64, u64)>,
        pub(super) net: Option<(u64, u64)>,
    }

    impl From<Counters> for Previous {
        fn from(counters: Counters) -> Self {
            Previous {
                at: Instant::now(),
                process_ticks: counters.process_ticks,
                cpu: counters.cpu,
                net: counters.net,
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn ticks_per_second() -> f64 {
        match rustix::param::clock_ticks_per_second() {
            0 => 100.0,
            ticks => ticks as f64,
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn ticks_per_second() -> f64 {
        100.0
    }

    /// Fills what `/proc` offers and returns the counters for rates.
    #[cfg(target_os = "linux")]
    pub(super) fn read(sample: &mut HostSample) -> Option<Previous> {
        let read = |path: &str| std::fs::read_to_string(path).ok();
        let status = read("/proc/self/status");
        if let Some(status) = status.as_deref() {
            sample.rss_bytes = status_kb(status, "VmRSS:");
            sample.vm_bytes = status_kb(status, "VmSize:");
            sample.threads = status_value(status, "Threads:");
        }
        sample.open_files = std::fs::read_dir("/proc/self/fd")
            .ok()
            .map(|entries| entries.count() as u64);
        sample.open_files_limit =
            read("/proc/self/limits").and_then(|text| open_files_limit(&text));
        let stat = read("/proc/stat");
        let cpu = stat.as_deref().map(cpu_times).unwrap_or_default();
        sample.cores = (cpu.len() > 1).then(|| cpu.len() as u64 - 1);
        sample.load = read("/proc/loadavg").and_then(|text| load_average(&text));
        if let Some(meminfo) = read("/proc/meminfo") {
            sample.memory_total = meminfo_kb(&meminfo, "MemTotal:");
            sample.memory_available = meminfo_kb(&meminfo, "MemAvailable:");
            sample.swap_total = meminfo_kb(&meminfo, "SwapTotal:");
            sample.swap_free = meminfo_kb(&meminfo, "SwapFree:");
        }
        if let Some(cgroup) = read("/proc/self/cgroup") {
            cgroup_figures(&cgroup, sample);
        }
        let process_ticks = read("/proc/self/stat").and_then(|text| process_ticks(&text));
        let net = read("/proc/net/dev").and_then(|text| net_bytes(&text));
        Some(
            Counters {
                process_ticks,
                cpu,
                net,
            }
            .into(),
        )
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn read(_sample: &mut HostSample) -> Option<Previous> {
        None
    }

    fn status_value(status: &str, key: &str) -> Option<u64> {
        status
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse().ok())
    }

    pub(super) fn status_kb(status: &str, key: &str) -> Option<u64> {
        status_value(status, key).map(|kb| kb * 1024)
    }

    pub(super) fn meminfo_kb(meminfo: &str, key: &str) -> Option<u64> {
        status_kb(meminfo, key)
    }

    /// The soft "Max open files" limit; None when unlimited.
    pub(super) fn open_files_limit(limits: &str) -> Option<u64> {
        let line = limits
            .lines()
            .find(|line| line.starts_with("Max open files"))?;
        line.trim_start_matches("Max open files")
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    }

    /// (busy, total) clock ticks of all cores (first) and of each core.
    pub(super) fn cpu_times(stat: &str) -> Vec<(u64, u64)> {
        stat.lines()
            .filter(|line| line.starts_with("cpu"))
            .filter_map(|line| {
                let values: Vec<u64> = line
                    .split_whitespace()
                    .skip(1)
                    .take(8)
                    .filter_map(|value| value.parse().ok())
                    .collect();
                if values.len() < 4 {
                    return None;
                }
                let total: u64 = values.iter().sum();
                let idle = values[3] + values.get(4).copied().unwrap_or(0);
                Some((total.saturating_sub(idle), total))
            })
            .collect()
    }

    pub(super) fn load_average(text: &str) -> Option<[f64; 3]> {
        let mut values = text
            .split_whitespace()
            .take(3)
            .filter_map(|value| value.parse::<f64>().ok());
        Some([values.next()?, values.next()?, values.next()?])
    }

    /// User plus system CPU ticks of this process from `/proc/self/stat`.
    pub(super) fn process_ticks(stat: &str) -> Option<u64> {
        // The command name may hold spaces and parentheses: fields start
        // after the last ')'. utime and stime are the 12th and 13th after it.
        let rest = &stat[stat.rfind(')')? + 1..];
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let utime: u64 = fields.get(11)?.parse().ok()?;
        let stime: u64 = fields.get(12)?.parse().ok()?;
        Some(utime + stime)
    }

    /// Bytes received and sent on every interface but loopback.
    pub(super) fn net_bytes(dev: &str) -> Option<(u64, u64)> {
        let mut total = None;
        for line in dev.lines().skip(2) {
            let Some((name, values)) = line.split_once(':') else {
                continue;
            };
            if name.trim() == "lo" {
                continue;
            }
            let values: Vec<u64> = values
                .split_whitespace()
                .filter_map(|value| value.parse().ok())
                .collect();
            if values.len() >= 9 {
                let (rx, tx) = total.get_or_insert((0, 0));
                *rx += values[0];
                *tx += values[8];
            }
        }
        total
    }

    /// The cgroup's memory use and limits. cgroup v2 names one path (`0::/…`);
    /// v1 one per controller. A limit set on an ancestor applies too, so the
    /// smallest along the path counts.
    #[cfg(target_os = "linux")]
    pub(super) fn cgroup_figures(cgroup: &str, sample: &mut HostSample) {
        let read = |path: &Path| std::fs::read_to_string(path).ok();
        let root = Path::new("/sys/fs/cgroup");
        if let Some(path) = cgroup.lines().find_map(|line| line.strip_prefix("0::")) {
            sample.cgroup = Some(path.to_string());
            let leaf = root.join(path.trim_start_matches('/'));
            sample.cgroup_memory = read(&leaf.join("memory.current"))
                .and_then(|value| value.trim().parse().ok())
                .or_else(|| {
                    read(&root.join("memory.current")).and_then(|value| value.trim().parse().ok())
                });
            let mut limit: Option<u64> = None;
            let mut cpu: Option<f64> = None;
            let mut dir = Some(leaf.as_path());
            while let Some(current) = dir {
                if !current.starts_with(root) {
                    break;
                }
                if let Some(value) =
                    read(&current.join("memory.max")).and_then(|text| limit_value(&text))
                {
                    limit = Some(limit.map_or(value, |known| known.min(value)));
                }
                if let Some(value) = read(&current.join("cpu.max")).and_then(|text| cpu_max(&text))
                {
                    cpu = Some(cpu.map_or(value, |known| known.min(value)));
                }
                dir = current.parent();
            }
            sample.cgroup_memory_limit = limit;
            sample.cgroup_cpu_limit = cpu;
            return;
        }
        // cgroup v1.
        let controller = |name: &str| {
            cgroup.lines().find_map(|line| {
                let mut parts = line.splitn(3, ':');
                let _ = parts.next();
                let controllers = parts.next()?;
                let path = parts.next()?;
                controllers
                    .split(',')
                    .any(|c| c == name)
                    .then(|| path.to_string())
            })
        };
        if let Some(path) = controller("memory") {
            sample.cgroup = Some(path.clone());
            let dir = root.join("memory").join(path.trim_start_matches('/'));
            sample.cgroup_memory = read(&dir.join("memory.usage_in_bytes"))
                .and_then(|value| value.trim().parse().ok());
            sample.cgroup_memory_limit =
                read(&dir.join("memory.limit_in_bytes")).and_then(|value| limit_value(&value));
        }
        if let Some(path) = controller("cpu") {
            let dir = root.join("cpu").join(path.trim_start_matches('/'));
            let quota: Option<i64> =
                read(&dir.join("cpu.cfs_quota_us")).and_then(|v| v.trim().parse().ok());
            let period: Option<i64> =
                read(&dir.join("cpu.cfs_period_us")).and_then(|v| v.trim().parse().ok());
            if let (Some(quota), Some(period)) = (quota, period) {
                if quota > 0 && period > 0 {
                    sample.cgroup_cpu_limit = Some(quota as f64 / period as f64);
                }
            }
        }
    }

    /// A memory limit; "max" and the v1 "unlimited" sentinel are no limit.
    pub(super) fn limit_value(text: &str) -> Option<u64> {
        let value: u64 = text.trim().parse().ok()?;
        (value < 1 << 60).then_some(value)
    }

    /// `cpu.max` ("max 100000" or "200000 100000") in cores.
    pub(super) fn cpu_max(text: &str) -> Option<f64> {
        let mut parts = text.split_whitespace();
        let quota: f64 = parts.next()?.parse().ok()?;
        let period: f64 = parts.next()?.parse().ok()?;
        (period > 0.0).then(|| quota / period)
    }
}

#[cfg(test)]
mod tests {
    use super::linux::*;

    #[test]
    fn proc_files_parse() {
        let stat = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 60 0 20 400 20 0 0 0 0 0\ncpu1 40 0 30 400 30 0 0 0 0 0\nintr 1";
        assert_eq!(cpu_times(stat), vec![(150, 1000), (80, 500), (70, 500)]);
        assert_eq!(
            load_average("0.52 0.40 0.31 2/345 6789\n"),
            Some([0.52, 0.40, 0.31])
        );
        let status = "Name:\tpockymoe\nVmSize:\t  2048 kB\nVmRSS:\t   512 kB\nThreads:\t17\n";
        assert_eq!(status_kb(status, "VmRSS:"), Some(512 * 1024));
        assert_eq!(status_kb(status, "VmSize:"), Some(2048 * 1024));
        let limits = "Limit                     Soft Limit           Hard Limit           Units\nMax open files            1024                 524288               files\n";
        assert_eq!(open_files_limit(limits), Some(1024));
        // A command name with spaces and a parenthesis.
        let own = "4242 (pocky moe) relay) S 1 4242 4242 0 -1 4194560 100 0 0 0 250 75 0 0 20 0 17 0 100 2097152 128";
        assert_eq!(process_ticks(own), Some(325));
        let dev = "Inter-|   Receive |  Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets\n    lo: 999 1 0 0 0 0 0 0 999 1 0 0 0 0 0 0\n  eth0: 1000 10 0 0 0 0 0 0 4000 20 0 0 0 0 0 0\n  wg0: 24 1 0 0 0 0 0 0 76 1 0 0 0 0 0 0\n";
        assert_eq!(net_bytes(dev), Some((1024, 4076)));
        assert_eq!(limit_value("max\n"), None);
        assert_eq!(limit_value("9223372036854771712\n"), None);
        assert_eq!(limit_value("536870912\n"), Some(536_870_912));
        assert_eq!(cpu_max("max 100000\n"), None);
        assert_eq!(cpu_max("150000 100000\n"), Some(1.5));
    }

    #[test]
    fn a_reading_never_fails_and_rates_follow_the_second() {
        let probe = super::HostProbe::default();
        let dir = std::env::temp_dir();
        let first = probe.sample(Some(&dir), 1);
        assert!(first.process_cpu_cores.is_none());
        if cfg!(unix) {
            assert!(first.disk_total.is_some_and(|total| total > 0));
        }
        if cfg!(target_os = "linux") {
            assert!(first.rss_bytes.is_some_and(|rss| rss > 0));
            assert!(first.cores.is_some_and(|cores| cores > 0));
            std::thread::sleep(std::time::Duration::from_millis(600));
            let second = probe.sample(Some(&dir), 2);
            assert!(second.process_cpu_cores.is_some());
            assert!(second.cpu.is_some());
        }
        assert_eq!(
            probe.latest().map(|(at, _)| at),
            Some(if cfg!(target_os = "linux") { 2 } else { 1 })
        );
    }
}
