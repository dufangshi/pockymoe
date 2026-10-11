//! Read-only Linux hwmon and powercap providers. Never use power limits as watts.
use super::hardware::{command, energy_watts, power, read_u64, unavailable};
use pockymoe_protocol::*;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct LinuxSensors {
    energy: HashMap<PathBuf, (Instant, u64)>,
    privileged_retry: Option<Instant>,
}

struct Counter {
    path: PathBuf,
    value: u64,
    max: Option<u64>,
    at: Instant,
}

fn package_paths(root: &Path) -> BTreeMap<String, Vec<PathBuf>> {
    let mut packages: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(true)
        .max_depth(3)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_name() != "energy_uj" {
            continue;
        }
        let Some(path) = entry.path().parent() else {
            continue;
        };
        let name = std::fs::read_to_string(path.join("name")).unwrap_or_default();
        let name = name.trim();
        if !name.starts_with("package-") {
            continue;
        }
        let Ok(path) = path.canonicalize() else {
            continue;
        };
        let paths = packages.entry(name.into()).or_default();
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    for paths in packages.values_mut() {
        // MSR and MMIO export the SAME physical package, sometimes through many
        // class symlinks. Prefer MSR, then any readable alternative; never sum them.
        paths.sort_by_key(|p| (p.to_string_lossy().contains("mmio"), p.clone()));
    }
    packages
}

impl LinuxSensors {
    async fn counters(&mut self, root: &Path) -> Result<Vec<Counter>, &'static str> {
        let packages = package_paths(root);
        if packages.is_empty() {
            return Err("No CPU package energy sensor exposed by the kernel (RAPL)");
        }
        let mut selected = vec![];
        let mut denied = vec![];
        for paths in packages.values() {
            if let Some((path, value)) = paths
                .iter()
                .find_map(|p| read_u64(p.join("energy_uj")).map(|v| (p.clone(), v)))
            {
                selected.push((path, value, Instant::now()));
            } else {
                denied.push(paths[0].clone());
            }
        }
        if !denied.is_empty() {
            // Only the fixed, kernel-owned sysfs counters are passed as arguments.
            // No shell, prompts, permission writes, or administrator installation.
            let can_probe = root == Path::new("/sys/class/powercap")
                && self
                    .privileged_retry
                    .is_none_or(|at| at.elapsed() >= Duration::from_secs(60));
            let files: Vec<String> = denied
                .iter()
                .map(|p| p.join("energy_uj").to_string_lossy().into())
                .collect();
            let mut args = vec!["-n", "/usr/bin/cat", "--"];
            args.extend(files.iter().map(String::as_str));
            let values = if can_probe {
                command("/usr/bin/sudo", &args).await
            } else {
                None
            };
            let values: Option<Vec<u64>> = values
                .as_deref()
                .map(|s| s.lines().map(|l| l.trim().parse().ok()).collect())
                .flatten();
            match values {
                Some(values) if values.len() == denied.len() => {
                    let now = Instant::now();
                    selected.extend(denied.into_iter().zip(values).map(|(p, v)| (p, v, now)));
                    self.privileged_retry = None;
                }
                _ => {
                    if can_probe {
                        self.privileged_retry = Some(Instant::now());
                    }
                    return Err("CPU package energy counters are unreadable; grant the Supervisor user read access to RAPL energy_uj files (see device-monitor setup)");
                }
            }
        }
        Ok(selected
            .into_iter()
            .map(|(path, value, at)| Counter {
                max: read_u64(path.join("max_energy_range_uj")),
                path,
                value,
                at,
            })
            .collect())
    }

    pub async fn power(&mut self, root: &Path, hwmon: &Path) -> PowerReadingDto {
        let mut counters = match self.counters(root).await {
            Ok(counters) => counters,
            Err(reason) => return hwmon_power(hwmon).unwrap_or_else(|| unavailable(reason)),
        };
        if counters.iter().any(|c| {
            self.energy
                .get(&c.path)
                .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(30))
        }) {
            self.energy.clear();
            for c in counters {
                self.energy.insert(c.path, (c.at, c.value));
            }
            // Warm the first/reopened view so it contains actual watts, rather than
            // needing a second hardware refresh ten seconds later.
            tokio::time::sleep(Duration::from_secs(1)).await;
            counters = match self.counters(root).await {
                Ok(counters) => counters,
                Err(reason) => return unavailable(reason),
            };
        }
        let mut total = 0.0;
        let mut complete = true;
        let active: std::collections::HashSet<_> =
            counters.iter().map(|c| c.path.clone()).collect();
        for c in counters {
            let old = self.energy.insert(c.path, (c.at, c.value));
            let watts = old.and_then(|(at, value)| {
                energy_watts(value, c.value, c.max, c.at.duration_since(at).as_secs_f64())
            });
            if let Some(watts) = watts {
                total += watts;
            } else {
                complete = false;
            }
        }
        self.energy.retain(|p, _| active.contains(p));
        if complete {
            power(Some(total), "Linux RAPL · CPU package energy delta")
        } else {
            unavailable("CPU package energy reading is incomplete or warming up")
        }
    }
}

fn hwmon_power(root: &Path) -> Option<PowerReadingDto> {
    let mut total = 0.0;
    let mut found = false;
    for entry in std::fs::read_dir(root).ok()?.filter_map(Result::ok) {
        let path = entry.path();
        let name = std::fs::read_to_string(path.join("name")).unwrap_or_default();
        if !matches!(name.trim(), "zenpower" | "fam15h_power") {
            continue;
        }
        let n = read_u64(path.join("power1_average"))
            .or_else(|| read_u64(path.join("power1_input")))?;
        total += n as f64 / 1_000_000.0;
        found = true;
    }
    found.then(|| power(Some(total), "Linux hwmon · CPU power"))
}

pub(super) fn temperature(root: &Path, thermal: &Path) -> TemperatureReadingDto {
    let mut sensors = vec![];
    let mut package = vec![];
    let mut cores = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        let driver = std::fs::read_to_string(path.join("name")).unwrap_or_default();
        if !matches!(
            driver.trim(),
            "coretemp" | "k10temp" | "zenpower" | "cpu_thermal"
        ) {
            continue;
        }
        // coretemp has one hwmon per package, labelled "Package id N", with
        // "Core <physical core id>" inputs.
        let mut package_id = 0;
        let mut core_readings = vec![];
        for e in std::fs::read_dir(&path)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(index) = name
                .strip_prefix("temp")
                .and_then(|s| s.strip_suffix("_input"))
            else {
                continue;
            };
            if index.is_empty() || !index.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Some(celsius) = std::fs::read_to_string(e.path())
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok())
                .and_then(|n| super::hardware::celsius(n / 1000.0))
            else {
                continue;
            };
            let label = std::fs::read_to_string(path.join(format!("temp{index}_label")))
                .unwrap_or_else(|_| driver.trim().into());
            let label = label.trim();
            if label.starts_with("Package id") || label == "Tdie" || label == "cpu_thermal" {
                package.push(celsius);
            }
            if let Some(id) = label
                .strip_prefix("Package id ")
                .and_then(|n| n.trim().parse().ok())
            {
                package_id = id;
            }
            if let Some(core) = label
                .strip_prefix("Core ")
                .and_then(|n| n.trim().parse::<u32>().ok())
            {
                core_readings.push((core, celsius));
            }
            sensors.push(TemperatureSensorDto {
                label: format!("{} · {label}", driver.trim()),
                celsius,
            });
        }
        cores.extend(
            core_readings
                .into_iter()
                .map(|(core, celsius)| ((package_id, core), celsius)),
        );
    }
    if sensors.is_empty() {
        for e in std::fs::read_dir(thermal)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            let path = e.path();
            let name = std::fs::read_to_string(path.join("type")).unwrap_or_default();
            if !matches!(name.trim(), "x86_pkg_temp" | "cpu-thermal" | "cpu_thermal") {
                continue;
            }
            if let Some(celsius) = std::fs::read_to_string(path.join("temp"))
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok())
                .and_then(|n| super::hardware::celsius(n / 1000.0))
            {
                sensors.push(TemperatureSensorDto {
                    label: name.trim().into(),
                    celsius,
                });
            }
        }
    }
    let celsius = if package.is_empty() {
        sensors.iter().map(|s| s.celsius).reduce(f64::max)
    } else {
        package.into_iter().reduce(f64::max)
    };
    TemperatureReadingDto {
        celsius, source: celsius.map(|_| "Linux hwmon / thermal · hottest CPU package or CPU sensor".into()),
        reason: celsius.is_none().then(|| "No readable CPU temperature sensor; the kernel must expose coretemp/k10temp or a CPU thermal zone".into()), sensors,
        cores,
    }
}

/// Gives each logical CPU the reading of the physical core it runs on, using
/// `cpu<N>/topology/{physical_package_id,core_id}`.
pub(super) fn attach_core_temperatures(
    cpu_root: &Path,
    readings: &std::collections::BTreeMap<(u32, u32), f64>,
    cores: &mut [CpuCoreMetricsDto],
) {
    if readings.is_empty() {
        return;
    }
    let read = |index: usize, name: &str| {
        std::fs::read_to_string(cpu_root.join(format!("cpu{index}/topology/{name}")))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
    };
    for core in cores {
        let Some(core_id) = read(core.index, "core_id") else {
            continue;
        };
        let package = read(core.index, "physical_package_id").unwrap_or(0);
        core.celsius = readings.get(&(package, core_id)).copied();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(root: &Path, name: &str, value: &str) {
        let p = root.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, value).unwrap();
    }
    #[tokio::test]
    async fn device_metrics_linux_deduplicates_msr_mmio_and_child_domains() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for provider in ["intel-rapl", "intel-rapl-mmio"] {
            let path = root.join(provider).join(format!("{provider}:0"));
            file(&path, "name", "package-0");
            file(&path, "energy_uj", "8000000");
            file(&path, "max_energy_range_uj", "100000000");
            file(&path.join("core"), "name", "core");
            file(&path.join("core"), "energy_uj", "90000000");
        }
        let packages = package_paths(root);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages["package-0"].len(), 2);
        let path = packages["package-0"][0].clone();
        assert!(!path.to_string_lossy().contains("mmio"));
        let mut collector = LinuxSensors::default();
        collector
            .energy
            .insert(path, (Instant::now() - Duration::from_secs(2), 0));
        let result = collector.power(root, &root.join("no-hwmon")).await;
        assert!((3.9..=4.1).contains(&result.watts.unwrap()), "{result:?}");
        assert_eq!(collector.energy.len(), 1);
        // Switching to MMIO after losing the MSR reading must warm the new counter,
        // rather than subtract values from two unrelated providers.
        std::fs::write(packages["package-0"][0].join("energy_uj"), "invalid").unwrap();
        let result = collector.power(root, &root.join("no-hwmon")).await;
        assert_eq!(result.watts, Some(0.0));
        assert_eq!(collector.energy.len(), 1);
    }
    #[test]
    fn device_metrics_linux_attaches_physical_core_temperatures_to_logical_cpus() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        file(root, "hwmon/hwmon1/name", "coretemp");
        file(root, "hwmon/hwmon1/temp1_label", "Package id 0");
        file(root, "hwmon/hwmon1/temp1_input", "80000");
        file(root, "hwmon/hwmon1/temp2_label", "Core 0");
        file(root, "hwmon/hwmon1/temp2_input", "70000");
        file(root, "hwmon/hwmon1/temp6_label", "Core 4");
        file(root, "hwmon/hwmon1/temp6_input", "84000");
        // Hybrid parts number physical cores sparsely: cpu1 runs on core 4.
        for (cpu, core) in [(0, "0"), (1, "4"), (2, "9")] {
            file(root, &format!("cpu/cpu{cpu}/topology/core_id"), core);
            file(
                root,
                &format!("cpu/cpu{cpu}/topology/physical_package_id"),
                "0",
            );
        }
        let reading = temperature(&root.join("hwmon"), &root.join("thermal"));
        let mut cores = (0..4)
            .map(|index| CpuCoreMetricsDto {
                index,
                usage_percent: None,
                celsius: None,
            })
            .collect::<Vec<_>>();
        attach_core_temperatures(&root.join("cpu"), &reading.cores, &mut cores);
        let celsius = cores.iter().map(|core| core.celsius).collect::<Vec<_>>();
        assert_eq!(celsius, [Some(70.0), Some(84.0), None, None]);
        // The field is omitted, not null, when a core has no reading.
        let json = serde_json::to_value(&cores[2]).unwrap();
        assert!(json.get("celsius").is_none());
    }
    #[test]
    fn device_metrics_linux_temperature_uses_package_not_ssd_or_acpi() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        file(root, "hwmon0/name", "nvme");
        file(root, "hwmon0/temp1_input", "120000");
        file(root, "hwmon1/name", "coretemp");
        file(root, "hwmon1/temp1_label", "Package id 0");
        file(root, "hwmon1/temp1_input", "97500");
        file(root, "hwmon1/temp2_label", "Core 0");
        file(root, "hwmon1/temp2_input", "99000");
        let result = temperature(root, &root.join("thermal"));
        assert_eq!(result.celsius, Some(97.5));
        assert_eq!(result.sensors.len(), 2);
        std::fs::write(root.join("hwmon1/temp1_input"), "999999").unwrap();
        assert_eq!(temperature(root, &root.join("thermal")).celsius, Some(99.0));
        std::fs::remove_dir_all(root.join("hwmon1")).unwrap();
        assert_eq!(temperature(root, &root.join("thermal")).celsius, None);
        file(root, "thermal/thermal_zone0/type", "x86_pkg_temp");
        file(root, "thermal/thermal_zone0/temp", "76000");
        assert_eq!(temperature(root, &root.join("thermal")).celsius, Some(76.0));
    }
    #[tokio::test]
    async fn device_metrics_linux_missing_counters_fall_back_to_cpu_hwmon_not_gpu_limits() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        file(root, "hwmon0/name", "amdgpu");
        file(root, "hwmon0/power1_average", "200000000");
        file(root, "hwmon1/name", "zenpower");
        file(root, "hwmon1/power1_average", "73400000");
        file(root, "hwmon1/power1_cap", "200000000");
        let mut collector = LinuxSensors::default();
        assert_eq!(
            collector.power(&root.join("no-powercap"), root).await.watts,
            Some(73.4)
        );
        std::fs::remove_file(root.join("hwmon1/power1_average")).unwrap();
        let result = collector.power(&root.join("no-powercap"), root).await;
        assert_eq!(result.watts, None);
        assert!(result
            .reason
            .unwrap()
            .contains("No CPU package energy sensor"));
    }
}
