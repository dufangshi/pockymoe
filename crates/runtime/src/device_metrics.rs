//! Device-local telemetry. Native CPU/RAM samples are shared across all readers;
//! optional sensor probes never turn unsupported/permission-denied values into zero.
mod hardware;
mod linux;
#[cfg(test)]
mod tests;

use anyhow::{anyhow, Result};
use pockymoe_protocol::*;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use sysinfo::{CpuRefreshKind, System};

#[derive(Default)]
pub struct DeviceMonitor {
    basic: Arc<Mutex<BasicCollector>>,
    hardware: tokio::sync::Mutex<hardware::HardwareCollector>,
}

#[derive(Default)]
struct BasicCollector {
    system: Option<System>,
    previous: Option<Instant>,
    cache: Option<(Instant, DeviceMetricsDto)>,
}

pub(super) fn percent(value: f64) -> Option<f64> {
    (value.is_finite() && (0.0..=100.0).contains(&value)).then_some(value)
}

pub(super) fn memory(total: u64, available: u64) -> MemoryMetricsDto {
    let available = available.min(total);
    let used = total - available;
    MemoryMetricsDto {
        total_bytes: total,
        used_bytes: used,
        available_bytes: available,
        usage_percent: (total > 0).then(|| used as f64 / total as f64 * 100.0),
    }
}

impl BasicCollector {
    fn sample(&mut self) -> DeviceMetricsDto {
        if let Some((at, data)) = &self.cache {
            if at.elapsed() < Duration::from_secs(2) {
                return data.clone();
            }
        }
        if self.system.is_none() {
            let mut system = System::new();
            system.refresh_cpu_specifics(CpuRefreshKind::nothing().with_cpu_usage());
            self.system = Some(system);
            self.previous = Some(Instant::now());
            // CPU utilization is a delta. Never publish the first since-boot
            // sample as "current", or refresh on every HTTP request.
            std::thread::sleep(Duration::from_secs(1));
        }
        let system = self.system.as_mut().unwrap();
        if self
            .previous
            .is_some_and(|at| at.elapsed() > Duration::from_secs(15))
        {
            // Re-opened monitors need a current sample, not an average over hours.
            system.refresh_cpu_specifics(CpuRefreshKind::nothing().with_cpu_usage());
            self.previous = Some(Instant::now());
            std::thread::sleep(Duration::from_secs(1));
        }
        system.refresh_cpu_specifics(CpuRefreshKind::nothing().with_cpu_usage());
        system.refresh_memory();
        let now = Instant::now();
        let window = self
            .previous
            .replace(now)
            .map(|at| now.duration_since(at).as_millis() as u64)
            .unwrap_or(0);
        let cores = system
            .cpus()
            .iter()
            .enumerate()
            .map(|(index, cpu)| CpuCoreMetricsDto {
                index,
                usage_percent: percent(cpu.cpu_usage() as f64),
                celsius: None,
            })
            .collect::<Vec<_>>();
        let data = DeviceMetricsDto {
            sampled_at: now_rfc3339(),
            sample_window_ms: window,
            platform: std::env::consts::OS.into(),
            environment: environment(),
            cpu: CpuMetricsDto {
                model: system
                    .cpus()
                    .first()
                    .map(|c| c.brand().to_string())
                    .unwrap_or_default(),
                usage_percent: (!cores.is_empty())
                    .then(|| percent(system.global_cpu_usage() as f64))
                    .flatten(),
                logical_core_count: cores.len(),
                cores,
            },
            memory: memory(system.total_memory(), system.available_memory()),
            swap: memory(
                system.total_swap(),
                system.total_swap().saturating_sub(system.used_swap()),
            ),
            cpu_power: PowerReadingDto::default(),
            cpu_temperature: TemperatureReadingDto::default(),
            gpus: vec![],
            hardware_sampled_at: String::new(),
            hardware_notes: vec![],
            limits: hardware::container_limits(),
        };
        self.cache = Some((now, data.clone()));
        data
    }
}

fn environment() -> String {
    if cfg!(target_os = "linux") {
        let release = std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_default()
            .to_lowercase();
        if release.contains("microsoft") {
            return "wsl".into();
        }
        if std::path::Path::new("/.dockerenv").exists()
            || std::path::Path::new("/run/.containerenv").exists()
            || std::fs::read_to_string("/proc/1/cgroup")
                .unwrap_or_default()
                .contains("docker")
        {
            return "container".into();
        }
    }
    "native".into()
}

impl DeviceMonitor {
    pub async fn snapshot(&self) -> Result<DeviceMetricsDto> {
        let basic = self.basic.clone();
        let basic_task = tokio::task::spawn_blocking(move || {
            basic
                .lock()
                .map_err(|_| anyhow!("Device collector lock failed"))
                .map(|mut c| c.sample())
        });
        let hardware_task = async { self.hardware.lock().await.sample().await };
        let (basic, hardware) = tokio::join!(basic_task, hardware_task);
        let mut basic = basic??;
        basic.cpu_power = hardware.cpu_power;
        if cfg!(target_os = "linux") {
            linux::attach_core_temperatures(
                std::path::Path::new("/sys/devices/system/cpu"),
                &hardware.cpu_temperature.cores,
                &mut basic.cpu.cores,
            );
        }
        basic.cpu_temperature = hardware.cpu_temperature;
        basic.gpus = hardware.gpus;
        basic.hardware_sampled_at = hardware.sampled_at;
        basic.hardware_notes = hardware.notes;
        Ok(basic)
    }
}
