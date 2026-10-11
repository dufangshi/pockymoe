use super::percent;
use pockymoe_protocol::*;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) struct HardwareSnapshot {
    pub sampled_at: String,
    pub cpu_power: PowerReadingDto,
    pub cpu_temperature: TemperatureReadingDto,
    pub gpus: Vec<GpuMetricsDto>,
    pub notes: Vec<String>,
}

#[derive(Default)]
pub(super) struct HardwareCollector {
    cache: Option<(Instant, HardwareSnapshot)>,
    linux: super::linux::LinuxSensors,
}

pub(super) fn unavailable(reason: &str) -> PowerReadingDto {
    PowerReadingDto {
        watts: None,
        source: None,
        reason: Some(reason.into()),
    }
}

pub(super) fn power(watts: Option<f64>, source: &str) -> PowerReadingDto {
    PowerReadingDto {
        watts,
        source: Some(source.into()),
        reason: watts.is_none().then(|| "Sensor reading unavailable".into()),
    }
}

fn number(s: &str) -> Option<f64> {
    let n = s.trim().parse::<f64>().ok()?;
    (n.is_finite() && n >= 0.0).then_some(n)
}

pub(super) fn celsius(value: f64) -> Option<f64> {
    (value.is_finite() && (-20.0..=150.0).contains(&value)).then_some(value)
}

fn temperature_unavailable(reason: &str) -> TemperatureReadingDto {
    TemperatureReadingDto {
        reason: Some(reason.into()),
        ..Default::default()
    }
}

fn native_temperature() -> TemperatureReadingDto {
    let components = sysinfo::Components::new_with_refreshed_list();
    let sensors: Vec<_> = components
        .iter()
        .filter_map(|c| {
            let label = c.label();
            // Do not mistake an SSD, battery, or generic SoC sensor for CPU die heat.
            if !label.to_lowercase().contains("cpu")
                && !label.starts_with("pACC")
                && !label.starts_with("eACC")
            {
                return None;
            }
            Some(TemperatureSensorDto {
                label: label.into(),
                celsius: celsius(c.temperature()? as f64)?,
            })
        })
        .collect();
    let value = sensors.iter().map(|s| s.celsius).reduce(f64::max);
    TemperatureReadingDto {
        celsius: value,
        source: value.map(|_| "macOS native SMC / IOHID · hottest CPU sensor".into()),
        reason: value
            .is_none()
            .then(|| "No CPU temperature exposed by the native SMC / IOHID sensors".into()),
        sensors,
        ..Default::default()
    }
}

pub(super) fn labeled_temperature(text: &str, label: &str) -> Option<f64> {
    text.lines().find_map(|line| {
        let mut parts = line.trim().strip_prefix(label)?.split_whitespace();
        let value = parts.next()?.parse().ok().and_then(celsius)?;
        matches!(parts.next()?, "C" | "°C").then_some(value)
    })
}

pub(super) async fn command(program: &str, args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let output = tokio::time::timeout(Duration::from_secs(5), cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() || output.stdout.len() > 1024 * 1024 {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn nvidia_program() -> Option<String> {
    which::which("nvidia-smi")
        .ok()
        .or_else(|| {
            [
                "/usr/lib/wsl/lib/nvidia-smi",
                "C:\\Windows\\System32\\nvidia-smi.exe",
                "C:\\Program Files\\NVIDIA Corporation\\NVSMI\\nvidia-smi.exe",
            ]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
        })
        .map(|p| p.to_string_lossy().into())
}

pub(super) fn parse_nvidia(text: &str) -> Vec<GpuMetricsDto> {
    // NVIDIA inserts spaces after delimiters, including before quoted names.
    let normalized = text.replace(", \"", ",\"");
    let mut csv = csv::ReaderBuilder::new()
        .has_headers(false)
        .trim(csv::Trim::All)
        .from_reader(normalized.as_bytes());
    csv.records()
        .filter_map(|row| {
            let row = row.ok()?;
            if row.len() != 6 || row[0].is_empty() {
                return None;
            }
            let bytes = |s: &str| {
                number(s)
                    .filter(|n| *n < u64::MAX as f64 / 1_048_576.0)
                    .map(|n| (n * 1_048_576.0) as u64)
            };
            Some(GpuMetricsDto {
                id: row[0].into(),
                name: row[1].into(),
                usage_percent: number(&row[2]).and_then(percent),
                used_memory_bytes: bytes(&row[3]),
                total_memory_bytes: bytes(&row[4]),
                power: power(number(&row[5]), "NVIDIA driver · board power"),
                source: "nvidia-smi".into(),
            })
        })
        .collect()
}

pub(super) fn energy_watts(
    previous: u64,
    current: u64,
    max: Option<u64>,
    seconds: f64,
) -> Option<f64> {
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    let delta = if current >= previous {
        current - previous
    } else {
        let max = max?;
        if previous > max || current > max {
            return None;
        }
        max - previous + current
    };
    Some(delta as f64 / 1_000_000.0 / seconds)
}

pub(super) fn read_u64(path: impl AsRef<Path>) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

impl HardwareCollector {
    pub async fn sample(&mut self) -> HardwareSnapshot {
        if let Some((at, sample)) = &self.cache {
            if at.elapsed() < Duration::from_secs(10) {
                return sample.clone();
            }
        }
        let nvidia = async {
            if let Some(program) = nvidia_program() {
                command(
                    &program,
                    &[
                        "--query-gpu=uuid,name,utilization.gpu,memory.used,memory.total,power.draw",
                        "--format=csv,noheader,nounits",
                    ],
                )
                .await
                .map(|s| parse_nvidia(&s))
                .unwrap_or_default()
            } else {
                vec![]
            }
        };
        let native = async {
            tokio::time::timeout(Duration::from_secs(7), async {
                match std::env::consts::OS {
                    "macos" => mac_sensors().await,
                    "windows" => windows_sensors().await,
                    _ => (
                        unavailable("CPU power sensors are unavailable"),
                        TemperatureReadingDto::default(),
                        vec![],
                    ),
                }
            })
            .await
            .unwrap_or_else(|_| {
                (
                    unavailable("Hardware sensor probe timed out"),
                    temperature_unavailable("Hardware sensor probe timed out"),
                    vec![],
                )
            })
        };
        let (mut gpus, (mut cpu_power, mut cpu_temperature, native_gpus)) =
            tokio::join!(nvidia, native);
        if gpus.is_empty() {
            gpus = native_gpus;
        }
        if cfg!(target_os = "linux") {
            cpu_temperature = super::linux::temperature(
                Path::new("/sys/class/hwmon"),
                Path::new("/sys/class/thermal"),
            );
            cpu_power = self
                .linux
                .power(
                    Path::new("/sys/class/powercap"),
                    Path::new("/sys/class/hwmon"),
                )
                .await;
            gpus.extend(linux_gpus());
        }
        let notes = if gpus.is_empty() {
            vec!["GPU metrics are unavailable on this device".into()]
        } else {
            vec![]
        };
        let sample = HardwareSnapshot {
            sampled_at: now_rfc3339(),
            cpu_power,
            cpu_temperature,
            gpus,
            notes,
        };
        self.cache = Some((Instant::now(), sample.clone()));
        sample
    }
}

fn linux_gpus() -> Vec<GpuMetricsDto> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return vec![];
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let label = entry.file_name().to_string_lossy().to_string();
            if !label
                .strip_prefix("card")
                .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            {
                return None;
            }
            let device = entry.path().join("device");
            let vendor = std::fs::read_to_string(device.join("vendor")).ok()?;
            if vendor.trim() == "0x10de" {
                return None;
            } // NVIDIA already sampled, never duplicate.
            let name = std::fs::read_to_string(device.join("product_name")).unwrap_or_else(|_| {
                format!(
                    "{} ({label})",
                    if vendor.trim() == "0x1002" {
                        "AMD GPU"
                    } else {
                        "GPU"
                    }
                )
            });
            let watts = std::fs::read_dir(device.join("hwmon"))
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .find_map(|e| {
                    read_u64(e.path().join("power1_average"))
                        .or_else(|| read_u64(e.path().join("power1_input")))
                })
                .map(|n| n as f64 / 1_000_000.0);
            Some(GpuMetricsDto {
                id: device
                    .canonicalize()
                    .unwrap_or(device.clone())
                    .to_string_lossy()
                    .into(),
                name: name.trim().into(),
                usage_percent: read_u64(device.join("gpu_busy_percent"))
                    .and_then(|n| percent(n as f64)),
                used_memory_bytes: read_u64(device.join("mem_info_vram_used")),
                total_memory_bytes: read_u64(device.join("mem_info_vram_total")),
                power: power(watts, "Linux hwmon · GPU power"),
                source: "Linux DRM".into(),
            })
        })
        .collect()
}

pub(super) fn labeled_power(text: &str, label: &str) -> Option<f64> {
    text.lines().find_map(|line| {
        let reading = line.trim().strip_prefix(label)?.trim();
        if let Some(value) = reading.strip_suffix("mW") {
            return number(value).map(|n| n / 1000.0);
        }
        number(reading.strip_suffix('W')?)
    })
}

pub(super) fn parse_ioreg(bytes: &[u8]) -> Vec<GpuMetricsDto> {
    fn visit(value: &plist::Value, output: &mut Vec<GpuMetricsDto>) {
        match value {
            plist::Value::Array(values) => {
                for value in values {
                    visit(value, output);
                }
            }
            plist::Value::Dictionary(values) => {
                if let Some(stats) = values
                    .get("PerformanceStatistics")
                    .and_then(plist::Value::as_dictionary)
                {
                    let usage = stats
                        .get("Device Utilization %")
                        .and_then(|v| {
                            v.as_unsigned_integer()
                                .map(|n| n as f64)
                                .or_else(|| v.as_real())
                        })
                        .and_then(percent);
                    let name = values
                        .get("IORegistryEntryName")
                        .and_then(plist::Value::as_string)
                        .unwrap_or("Apple GPU");
                    output.push(GpuMetricsDto {
                        id: format!("ioreg-{name}-{}", output.len()),
                        name: name.into(),
                        usage_percent: usage,
                        used_memory_bytes: None,
                        total_memory_bytes: None,
                        power: unavailable("GPU power needs powermetrics permission"),
                        source: "macOS IORegistry".into(),
                    });
                }
                for value in values.values() {
                    visit(value, output);
                }
            }
            _ => {}
        }
    }
    let mut gpus = vec![];
    if let Ok(value) = plist::Value::from_reader_xml(std::io::Cursor::new(bytes)) {
        visit(&value, &mut gpus);
    }
    gpus
}

async fn mac_sensors() -> (PowerReadingDto, TemperatureReadingDto, Vec<GpuMetricsDto>) {
    let powermetrics = async {
        let preferred = if cfg!(target_arch = "aarch64") {
            "cpu_power,gpu_power"
        } else {
            "cpu_power,smc"
        };
        for samplers in [preferred, "cpu_power"] {
            let args = ["--samplers", samplers, "-i", "1000", "-n", "1"];
            if let Some(text) = command("/usr/bin/powermetrics", &args).await {
                return Some(text);
            }
            // Only use administrator access that is already authorized. A missing
            // sampler must not hide CPU watts that the cpu_power sampler supports.
            let args = [
                "-n",
                "/usr/bin/powermetrics",
                "--samplers",
                samplers,
                "-i",
                "1000",
                "-n",
                "1",
            ];
            if let Some(text) = command("/usr/bin/sudo", &args).await {
                return Some(text);
            }
        }
        None
    };
    let registry = async {
        for class in ["IOAccelerator", "IOGPU"] {
            if let Some(text) =
                command("/usr/sbin/ioreg", &["-a", "-r", "-c", class, "-d", "2"]).await
            {
                let gpus = parse_ioreg(text.as_bytes());
                if !gpus.is_empty() {
                    return gpus;
                }
            }
        }
        vec![]
    };
    let temperature = tokio::task::spawn_blocking(native_temperature);
    let (text, mut gpus, temperature) = tokio::join!(powermetrics, registry, temperature);
    let mut temperature =
        temperature.unwrap_or_else(|_| temperature_unavailable("Native temperature probe failed"));
    let Some(text) = text else {
        return (
            unavailable("powermetrics is unavailable or needs administrator access for CPU power"),
            temperature,
            gpus,
        );
    };
    let cpu = labeled_power(&text, "CPU Power:");
    let package = labeled_power(&text, "Package Power:").or_else(|| {
        labeled_power(
            &text,
            "Intel energy model derived package power (CPUs+GT+SA):",
        )
    });
    let cpu_power = power(
        cpu.or(package),
        if cpu.is_some() {
            "macOS powermetrics · CPU"
        } else {
            "macOS powermetrics · package"
        },
    );
    if let Some(watts) = labeled_power(&text, "GPU Power:") {
        if gpus.is_empty() {
            gpus.push(GpuMetricsDto {
                id: "apple-gpu".into(),
                name: "Apple GPU".into(),
                usage_percent: None,
                used_memory_bytes: None,
                total_memory_bytes: None,
                power: PowerReadingDto::default(),
                source: "powermetrics".into(),
            });
        }
        // powermetrics reports aggregate GPU power, not a per-adapter value.
        if gpus.len() == 1 {
            gpus[0].power = power(Some(watts), "macOS powermetrics · GPU");
        }
    }
    if temperature.celsius.is_none() {
        if let Some(value) = labeled_temperature(&text, "CPU die temperature:") {
            temperature = TemperatureReadingDto {
                celsius: Some(value),
                source: Some("macOS powermetrics · CPU die".into()),
                reason: None,
                sensors: vec![],
                ..Default::default()
            };
        }
    }
    (cpu_power, temperature, gpus)
}

const WINDOWS_SENSORS: &str = r#"
$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[System.Text.UTF8Encoding]::new();
$cpu=$null; try { $sensors=@(Get-CimInstance -Namespace root/LibreHardwareMonitor -ClassName Sensor | Where-Object { $_.SensorType -eq 'Power' -and $_.Name -eq 'CPU Package' }); if ($sensors.Count -gt 0) { $cpu=($sensors | Measure-Object Value -Sum).Sum } } catch {}
$temp=$null; try { $ts=@(Get-CimInstance -Namespace root/LibreHardwareMonitor -ClassName Sensor | Where-Object { $_.SensorType -eq 'Temperature' -and $_.Name -match '^CPU (Package|Tdie)$' }); if ($ts.Count -gt 0) { $temp=($ts | Measure-Object Value -Maximum).Maximum } } catch {}
$gpu=@(); try {
 $engines=Get-CimInstance -ClassName Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine;
 $sums=$engines | Group-Object { $_.Name -replace '^pid_\d+_','' -replace '_engtype_.*$','' } | ForEach-Object { @{ id=($_.Name -replace '_eng_.*$',''); load=($_.Group | Measure-Object UtilizationPercentage -Sum).Sum } };
 $gpu=@($sums | Group-Object id | ForEach-Object { @{ id=$_.Name; name=('GPU '+$_.Name); usagePercent=[Math]::Min(100,($_.Group | Measure-Object load -Maximum).Maximum) } });
} catch {}
@{cpuPowerWatts=$cpu;cpuTemperatureCelsius=$temp;gpus=$gpu} | ConvertTo-Json -Depth 5 -Compress
"#;

pub(super) fn parse_windows(
    text: &str,
) -> (PowerReadingDto, TemperatureReadingDto, Vec<GpuMetricsDto>) {
    let value: serde_json::Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default();
    let watts = value["cpuPowerWatts"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0);
    let cpu = if watts.is_some() {
        power(watts, "LibreHardwareMonitor · CPU package")
    } else {
        unavailable("CPU power needs a hardware sensor provider")
    };
    let gpus = value["gpus"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            Some(GpuMetricsDto {
                id: v["id"].as_str()?.into(),
                name: v["name"].as_str()?.into(),
                usage_percent: v["usagePercent"].as_f64().and_then(percent),
                used_memory_bytes: None,
                total_memory_bytes: None,
                power: unavailable("GPU power sensor unavailable"),
                source: "Windows GPU engine counters (busiest engine)".into(),
            })
        })
        .collect();
    let degrees = value["cpuTemperatureCelsius"].as_f64().and_then(celsius);
    let temperature = TemperatureReadingDto {
        celsius: degrees,
        source: degrees.map(|_| "LibreHardwareMonitor · CPU package".into()),
        reason: degrees
            .is_none()
            .then(|| "CPU temperature needs a hardware sensor provider".into()),
        sensors: vec![],
        ..Default::default()
    };
    (cpu, temperature, gpus)
}

async fn windows_sensors() -> (PowerReadingDto, TemperatureReadingDto, Vec<GpuMetricsDto>) {
    command(
        "powershell.exe",
        &[
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            WINDOWS_SENSORS,
        ],
    )
    .await
    .map(|s| parse_windows(&s))
    .unwrap_or_else(|| {
        (
            unavailable("Hardware sensor query unavailable"),
            temperature_unavailable("Hardware sensor query unavailable"),
            vec![],
        )
    })
}

pub(super) fn container_limits() -> Option<DeviceLimitsDto> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let memory_total_bytes = read_u64("/sys/fs/cgroup/memory.max")
        .or_else(|| read_u64("/sys/fs/cgroup/memory/memory.limit_in_bytes"))
        .filter(|n| *n < (1u64 << 60));
    let memory_used_bytes = read_u64("/sys/fs/cgroup/memory.current")
        .or_else(|| read_u64("/sys/fs/cgroup/memory/memory.usage_in_bytes"));
    let cpu_cores = std::fs::read_to_string("/sys/fs/cgroup/cpu.max")
        .ok()
        .and_then(|s| {
            let mut p = s.split_whitespace();
            let quota = number(p.next()?)?;
            let period = number(p.next()?)?;
            (period > 0.0).then_some(quota / period)
        });
    (memory_total_bytes.is_some() || cpu_cores.is_some()).then_some(DeviceLimitsDto {
        cpu_cores,
        memory_total_bytes,
        memory_used_bytes,
    })
}
