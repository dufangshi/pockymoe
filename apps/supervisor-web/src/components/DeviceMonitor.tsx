import { getLocale } from '@pockymoe/thread-ui/i18n';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { type CSSProperties, useEffect, useState } from 'react';
import type { DeviceMetricsDto, PowerReadingDto, TemperatureReadingDto } from '@pockymoe/shared';
import { ApiError, request } from '../lib/api';
import { FormDialog } from './FormDialog';
import './device-monitor.css';

const pct = (value: number | null | undefined) =>
  value == null ? '—' : `${Math.round(value)}%`;
// Exposes a live reading as --load (0–100) so a theme can color it by load.
const load = (value: number | null | undefined) =>
  value == null ? undefined : ({ '--load': Math.min(100, Math.max(0, Math.round(value))) } as CSSProperties);
const gib = (bytes: number) => `${(bytes / 2 ** 30).toFixed(1)} GiB`;
function Power({
  label,
  reading,
}: {
  label: string;
  reading: PowerReadingDto;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <>
    <div className="device-monitor-reading">
      <span title={reading.source ?? undefined}>{label}</span>
      <span title={reading.reason ?? reading.source ?? undefined}>
        {reading.watts == null
          ? translate("devices.unavailable")
          : `${reading.watts.toFixed(1)} W`}
      </span>
    </div>
      {reading.watts == null && reading.reason && (
        <p className="device-monitor-caption">
          {reading.reason}{' '}
          {reading.reason.includes('RAPL') && (
            <a href="https://github.com/dufangshi/pockymoe/blob/main/docs/device-monitor.md#linux-sensor-access" target="_blank" rel="noreferrer">
              {translate("devices.sensorAccessSetup")}</a>
          )}
        </p>
      )}
    </>
  );
}

function Temperature({ reading }: { reading: TemperatureReadingDto | undefined }) {
  const { locale: i18nLocale } = useI18n();
  return (
    <>
      <div className="device-monitor-reading">
        <span>{translate("devices.cPUTemperature")}</span>
        <span title={reading?.source ?? undefined}>
          {reading?.celsius == null ? translate("devices.unavailable") : `${reading.celsius.toFixed(1)} °C`}
        </span>
      </div>
      {reading?.celsius == null && (
        <p className="device-monitor-caption">
          {reading?.reason ?? translate("devices.updateThisDeviceSSupervisorToCollect")}
        </p>
      )}
      {Boolean(reading?.sensors.length) && (
        <details className="device-monitor-sensors">
          <summary>{translate("devices.cPUTemperatureSensors")}{reading!.sensors.length})</summary>
          {reading!.sensors.map((sensor, index) => (
            <div className="device-monitor-reading" key={`${sensor.label}-${index}`}>
              <span>{sensor.label}</span><span>{sensor.celsius.toFixed(1)} °C</span>
            </div>
          ))}
        </details>
      )}
    </>
  );
}

export function DeviceMonitor() {
  const { locale: i18nLocale } = useI18n();
  const [open, setOpen] = useState(false);
  const [data, setData] = useState<DeviceMetricsDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [forbidden, setForbidden] = useState(false);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    let disposed = false;
    let busy = false;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    let controller: AbortController | null = null;
    const poll = async () => {
      if (disposed || busy || stopped) return;
      clearTimeout(timer);
      if (document.hidden || !navigator.onLine) {
        if (!navigator.onLine) setError(translate("devices.deviceMonitoringIsOffline"));
        timer = setTimeout(poll, 5000);
        return;
      }
      busy = true;
      controller = new AbortController();
      const timeout = setTimeout(() => controller?.abort(), 10_000);
      try {
        const sample = await request<DeviceMetricsDto>('/api/device/metrics', {
          signal: controller.signal,
        });
        if (!disposed) {
          setData(sample);
          setError(null);
        }
      } catch (cause) {
        if (!disposed) {
          if (cause instanceof ApiError && cause.statusCode === 403) {
            setForbidden(true);
            stopped = true;
          } else if (cause instanceof ApiError && cause.statusCode === 404) {
            setError(translate("devices.updateThisDeviceSSupervisorToEnable"));
            // Check occasionally after the device is updated.
          } else
            setError(translate("devices.deviceMetricsUnavailableShowingTheLastSample"));
        }
      } finally {
        clearTimeout(timeout);
        busy = false;
        if (!disposed && !stopped) timer = setTimeout(poll, open ? 2000 : 5000);
      }
    };
    void poll();
    const wake = () => {
      if (!document.hidden) void poll();
    };
    document.addEventListener('visibilitychange', wake);
    window.addEventListener('online', wake);
    window.addEventListener('offline', wake);
    const clock = setInterval(() => setNow(Date.now()), 1000);
    return () => {
      disposed = true;
      clearTimeout(timer);
      clearInterval(clock);
      controller?.abort();
      document.removeEventListener('visibilitychange', wake);
      window.removeEventListener('online', wake);
      window.removeEventListener('offline', wake);
    };
  }, [open]);
  if (forbidden) return null;
  const age = data
    ? Math.max(0, Math.floor((now - Date.parse(data.sampledAt)) / 1000))
    : null;
  const stale = Boolean(error) || (age != null && age > 15);
  const gpu = data?.gpus.find((gpu) => gpu.usagePercent != null);
  return (
    <>
      <button
        type="button"
        className="device-monitor-button"
        aria-label={translate("devices.deviceMonitor")}
        aria-expanded={open}
        data-stale={stale || undefined}
        title={error ?? translate("devices.cPUMemoryAndHardwareSensors")}
        onClick={() => setOpen(true)}
      >
        <span>
          CPU <b style={load(stale ? null : data?.cpu.usagePercent)}>{pct(stale ? null : data?.cpu.usagePercent)}</b>
        </span>
        <span>
          RAM <b style={load(stale ? null : data?.memory.usagePercent)}>{pct(stale ? null : data?.memory.usagePercent)}</b>
        </span>
        {gpu && (
          <span>
            GPU <b style={load(stale ? null : gpu.usagePercent)}>{pct(stale ? null : gpu.usagePercent)}</b>
          </span>
        )}
      </button>
      {open && (
        <FormDialog
          title={translate("devices.deviceMonitor")}
          onClose={() => setOpen(false)}
          description={
            data?.environment === 'wsl'
              ? translate("devices.wSLEnvironmentCPUAndMemoryDescribeThe")
              : data?.environment === 'container'
                ? translate("devices.containerEnvironmentSystemCPUAndMemoryWith")
                : translate("devices.currentSystemCPUMemoryAndAvailableHardware")
          }
        >
          <div className="device-monitor-details">
            {error && (
              <p role="status" className="device-monitor-notice">
                {error}
              </p>
            )}
            {!data && !error && (
              <p role="status">{translate("devices.collectingTheFirstCPUSample")}</p>
            )}
            {data && (
              <>
                <p className="device-monitor-caption">
                  {stale ? translate("devices.lastSample") : translate("devices.updated")} {age}{translate("devices.sAgo")}{' '}
                  {data.platform}
                  <br />
                  {translate("devices.cPUSampleWindow")} {(data.sampleWindowMs / 1000).toFixed(1)}{translate("devices.s")}</p>
                <section aria-label={translate("devices.cPUUtilization")}>
                  <div className="device-monitor-heading">
                    <h3>CPU</h3>
                    <strong>{pct(data.cpu.usagePercent)}</strong>
                  </div>
                  <p className="device-monitor-caption">
                    {data.cpu.model} · {data.cpu.logicalCoreCount} {translate("devices.logicalCores")}</p>
                  <div className="device-monitor-cores">
                    {data.cpu.cores.map((core) => (
                      <div key={core.index} className="device-monitor-core">
                        <span>
                          {translate("devices.core")} {core.index + 1}
                          <b>{pct(core.usagePercent)}</b>
                        </span>
                        <progress
                          aria-label={translate("devices.coreUtilization", { value1: core.index + 1 })}
                          max={100}
                          value={core.usagePercent ?? undefined}
                        />
                      </div>
                    ))}
                  </div>
                  <Power label={translate("devices.cPUPower")} reading={data.cpuPower} />
                  <Temperature reading={data.cpuTemperature} />
                </section>
                <section aria-label={translate("devices.memoryUtilization")}>
                  <div className="device-monitor-heading">
                    <h3>{translate("devices.memory")}</h3>
                    <strong>{pct(data.memory.usagePercent)}</strong>
                  </div>
                  <div className="device-monitor-reading">
                    <span>{translate("devices.usedTotal")}</span>
                    <span>
                      {gib(data.memory.usedBytes)} /{' '}
                      {gib(data.memory.totalBytes)}
                    </span>
                  </div>
                  <div className="device-monitor-reading">
                    <span>{translate("devices.available")}</span>
                    <span>{gib(data.memory.availableBytes)}</span>
                  </div>
                  {data.swap.totalBytes > 0 && (
                    <div className="device-monitor-reading">
                      <span>{translate("devices.swapPageFile")}</span>
                      <span>
                        {gib(data.swap.usedBytes)} / {gib(data.swap.totalBytes)}
                      </span>
                    </div>
                  )}
                </section>
                {data.limits && (
                  <section aria-label={translate("devices.containerLimits")}>
                    <h3>{translate("devices.containerLimits")}</h3>
                    {data.limits.cpuCores != null && (
                      <div className="device-monitor-reading">
                        <span>{translate("devices.cPUQuota")}</span>
                        <span>{data.limits.cpuCores} {translate("devices.cores")}</span>
                      </div>
                    )}
                    {data.limits.memoryTotalBytes != null && (
                      <div className="device-monitor-reading">
                        <span>{translate("devices.memory")}</span>
                        <span>
                          {data.limits.memoryUsedBytes == null
                            ? '—'
                            : gib(data.limits.memoryUsedBytes)}{' '}
                          / {gib(data.limits.memoryTotalBytes)}
                        </span>
                      </div>
                    )}
                  </section>
                )}
                {data.gpus.map((gpu) => (
                  <section key={gpu.id} aria-label={`GPU ${gpu.name}`}>
                    <div className="device-monitor-heading">
                      <h3>{gpu.name}</h3>
                      <strong>{pct(gpu.usagePercent)}</strong>
                    </div>
                    {gpu.totalMemoryBytes != null && (
                      <div className="device-monitor-reading">
                        <span>{translate("devices.gPUMemory")}</span>
                        <span>
                          {gpu.usedMemoryBytes == null
                            ? '—'
                            : gib(gpu.usedMemoryBytes)}{' '}
                          / {gib(gpu.totalMemoryBytes)}
                        </span>
                      </div>
                    )}
                    <Power label={translate("devices.gPUPower")} reading={gpu.power} />
                  </section>
                ))}
                {data.hardwareNotes.map((note) => (
                  <p key={note} className="device-monitor-caption">
                    {note}
                  </p>
                ))}
                <p className="device-monitor-caption">
                  {translate("devices.hardwareSensorsSampledAt")}{' '}
                  {new Date(data.hardwareSampledAt).toLocaleTimeString(getLocale())}{translate("devices.powerRequiresSupportedHardwareAndSensorAccess")}</p>
              </>
            )}
          </div>
        </FormDialog>
      )}
    </>
  );
}
