# Device monitoring

`GET /api/device/metrics` returns a camelCase `DeviceMetricsDto`. The device
Supervisor collects CPU and memory locally, and the relay forwards the endpoint
through normal authenticated transport. Owners and device-scoped grants may read
it; thread/workspace-scoped grants cannot inspect the host.

CPU percentages use native counter differences through `sysinfo`, shared between
readers. First use, and reopening after a pause longer than 15 seconds, warm the
counter for one second. `sampleWindowMs` describes the averaging interval.
Basic readings cache for two seconds; optional hardware sensors cache for ten.
There is no always-running collector when nobody is viewing the device.

Memory uses native total and available physical RAM in bytes; used memory is
total minus available, including reclaimable memory according to the OS. Swap
or the page file is separate. WSL reports the Linux VM rather than Windows host
RAM. Containers expose OS-visible CPU/RAM plus available cgroup limits separately.

| System  | CPU and RAM                             | Optional GPU/power sources                                                                                                             |
| ------- | --------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| Linux   | Native system counters, per logical CPU | NVIDIA driver; DRM utilization/VRAM and hwmon GPU watts; RAPL CPU package energy deltas; coretemp/k10temp temperatures                                                |
| Windows | Native system counters, per logical CPU | NVIDIA driver; WDDM engine counters (busiest engine per adapter); CPU package watts/temperature from an existing LibreHardwareMonitor WMI provider |
| macOS   | Native system counters, per logical CPU | IORegistry GPU utilization; native SMC/IOHID CPU temperatures; `powermetrics` CPU/GPU watts if accessible                                                                 |
| WSL     | Linux VM counters, per logical CPU      | WSL NVIDIA driver where supported; host power is usually unavailable                                                                   |

Power is measured watts, never thermal design power or an estimate from CPU
utilization. Package/board readings can include more than CPU/GPU compute units;
they are not total device power and must not be added to estimate wall power.
Unsupported, malformed, permission-denied or timed-out sensor readings are null,
with a reason. Zero remains a valid measured value. RAPL collects only package
domains and handles one counter wrap; long gaps re-prime rather than guess wraps.
The app does not install drivers, change sensor permissions or ask for a password.
macOS can use an already-authorized noninteractive `sudo -n` for powermetrics.
Linux can similarly read the specific energy counters through already-authorized
`sudo -n /usr/bin/cat -- ...`; failed privileged probes back off for one minute.
MSR and MMIO representations of the same `package-N` are alternatives, never
added together. CPU hwmon power (`zenpower` / `fam15h_power`) is a fallback. The
first/reopened readable RAPL sample warms for one second and returns watts.

CPU temperature uses CPU-only hwmon drivers or CPU thermal zones on Linux;
package/Tdie values take precedence over core values. SSD, Wi-Fi and generic ACPI
thermal zones are excluded. macOS uses sysinfo’s native Intel SMC / Apple Silicon
IOHID reader, with Intel powermetrics SMC temperatures as a fallback. Windows
uses CPU Package/Tdie temperatures from LibreHardwareMonitor when installed.
`cpuTemperature` includes Celsius, source/reason and individually labeled sensors.
On Linux, coretemp's `Core <id>` readings are matched to logical CPUs through
`/sys/devices/system/cpu/cpu<N>/topology/{physical_package_id,core_id}` and sent as
`cpu.cores[].celsius`; hybrid Intel parts number physical cores sparsely, so the
order of the sensors is not the order of the CPUs. Each core chart shows its
temperature; without per-core readings the UI lists the sensors under the
expanded CPU row instead. An unavailable reading shows
a small "not available" mark whose tooltip and accessible name carry the reason;
a missing RAPL permission links the mark to the setup below.

The compact monitor pairs CPU and physical memory (`DM`, DRAM) on its first row.
When a GPU reports utilization or memory, a second row pairs GPU utilization and
video memory (`VM`, VRAM). VM is used/total GPU memory, not GPU compute load;
missing readings show a dash. CPU-only devices use a single, slightly larger row.

## Linux sensor access

RAPL `energy_uj` is commonly root-readable only. An interface being present does
not imply the Supervisor account can read it. Neither switching from sysfs to
MSR/perf nor installing `sensors` bypasses that permission. Do not grant blanket
root access to the Supervisor, expose all MSRs, or use TDP/limits as measured power.

An administrator can grant the Supervisor’s existing private group read access
to just the CPU package counter. For the audited Ubuntu host (user/group
`ubuntu`, package `intel-rapl:0`), the commands are:

```bash
sudo chgrp ubuntu /sys/class/powercap/intel-rapl:0/energy_uj
sudo chmod g+r /sys/class/powercap/intel-rapl:0/energy_uj
```

For other hosts, first verify the service user/group and each zone’s `name`;
authorize each physical package. These sysfs permissions reset on boot. For a
persistent setup, an administrator can install a narrow udev rule (substitute
the intended group) and trigger existing powercap devices:

```udev
ACTION=="add", SUBSYSTEM=="powercap", KERNEL=="intel-rapl:*", ATTR{name}=="package-*", RUN+="/usr/bin/chgrp ubuntu /sys%p/energy_uj", RUN+="/usr/bin/chmod g+r /sys%p/energy_uj"
```

Save under `/etc/udev/rules.d/99-pockymoe-rapl.rules`, then run
`sudo udevadm control --reload-rules` and
`sudo udevadm trigger --subsystem-match=powercap --action=add`. The collector
retries ordinary reads on the next hardware sample, without a Supervisor restart.

## macOS sensor access and limits

`powermetrics` is an Apple-provided interface but normally requires administrator
access. The collector chooses Apple Silicon CPU/GPU samplers or Intel CPU/SMC
samplers, with a CPU-only fallback if optional samplers are absent. An existing,
restricted sudo policy for the exact read-only command can permit the fallback;
Pockymoe never prompts for a password or edits that policy. Native
temperature availability varies by chip/macOS version, even with administrator
access; absent sensors remain null. IOReport Energy Model can provide unprivileged
Apple Silicon energy data on some versions, but is a private ABI; this collector
does not claim support or infer watts from unrelated channels.

References: [kernel powercap/RAPL](https://www.kernel.org/doc/html/latest/power/powercap/powercap.html),
[kernel coretemp](https://www.kernel.org/doc/html/latest/hwmon/coretemp.html),
[sysinfo native Apple sensor implementation](https://github.com/GuillaumeGomez/sysinfo/tree/v0.36.1/src/unix/apple/macos/component),
[Netdata macOS sensor sources and privilege requirements](https://learn.netdata.cloud/docs/collecting-metrics/collectors/operating-systems/macos).

The Web button appears in the mobile topbar and desktop icon rail above Settings,
remaining visible when the chat sidebar is collapsed. Opening it shows the sample
age and platform beside the title, then one row per resource (CPU, memory with
swap, each GPU, container limits): an icon, the value and a meter colored from
green to red by load, with temperature and power as small chips. The CPU row
expands into a usage chart with a 0–100% scale and the current reading, and a
per-core chart grid, drawn from about the last 30 samples the page has polled. A failed or stale sample replaces button
percentages with dashes; the details retain the last sample with its age. The
topbar meter samples every 15 seconds and the open dialog every 2 seconds; hidden
browser tabs pause polling and sample again on return. Older Supervisors display
an update instruction instead of fabricated zeroes.

Validation covers Linux native samples against `/proc/stat` and `/proc/meminfo`,
concurrent reader caching, the isolated HTTP endpoint, relay scope restrictions,
and Windows/macOS/NVIDIA provider fixtures. Browser regressions cover both desktop
and mobile (including 320px width), details, keyboard dismissal and stale samples.
Sensor availability on real hardware remains conditional on its drivers and access.

# Turn progress indicator

Running turns retain three animated dots. `Last progress · Ns ago` shows the age
of the last received text, plan, tool, context, subagent, request or turn/usage
event for that turn. Socket pings, polling, expanding history and rerendering do
not reset it. Initial/reloaded views fall back to recorded activity timestamps.
Silence does not itself mark a turn stuck or finished; known background agents
continue to have their own explicit status.
