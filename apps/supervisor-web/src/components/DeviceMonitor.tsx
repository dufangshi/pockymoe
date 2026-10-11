import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { type CSSProperties, type ReactNode, useEffect, useId, useState } from 'react';
import { ArrowLeftRight, Ban, ChevronDown, Container, Cpu, Gpu, Info, MemoryStick, Thermometer, TriangleAlert, Zap } from 'lucide-react';
import type { DeviceMetricsDto, TemperatureReadingDto } from '@pockymoe/shared';
import { ApiError, request } from '../lib/api';
import { FormDialog } from './FormDialog';
import './device-monitor.css';

const pct = (value: number | null | undefined) =>
  value == null ? '—' : `${Math.round(value)}%`;
const clampLoad = (value: number) => Math.min(100, Math.max(0, Math.round(value)));
// Exposes a live reading as --load (0–100) so the meters and themes can color it
// by load.
const load = (value: number | null | undefined) =>
  value == null ? undefined : ({ '--load': clampLoad(value) } as CSSProperties);
const gib = (bytes: number) => `${(bytes / 2 ** 30).toFixed(1)} GiB`;
const SENSOR_SETUP = 'https://github.com/dufangshi/pockymoe/blob/main/docs/device-monitor.md#linux-sensor-access';
/** Samples kept for the CPU charts: about a minute while the dialog polls. */
const HISTORY = 30;
/** The open dialog samples every 2 s; the topbar meter every 15 s. */
const OPEN_POLL_MS = 2_000;
const CLOSED_POLL_MS = 15_000;
/** A sample older than about two intervals is shown as stale. */
const staleAfterSeconds = (open: boolean) => (open ? 15 : 35);
type CpuSample = { at: string; total: number | null; cores: Array<number | null> };

function Meter({ value, label, thin = false }: { value: number | null; label: string; thin?: boolean }) {
  return (
    <div
      className={`dm-meter${thin ? ' is-thin' : ''}`}
      role="meter"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={value == null ? undefined : clampLoad(value)}
      style={load(value)}
    >
      <i style={{ width: `${value == null ? 0 : clampLoad(value)}%` }} />
    </div>
  );
}

/** A small "not available" mark; the reason stays in its tooltip and name. */
function Unavailable({ reason }: { reason?: string | null | undefined }) {
  const text = reason ? `${translate('devices.unavailable')}: ${reason}` : translate('devices.unavailable');
  return reason?.includes('RAPL') ? (
    <a className="dm-na" href={SENSOR_SETUP} target="_blank" rel="noreferrer" title={text} aria-label={text}>
      <Ban aria-hidden />
    </a>
  ) : (
    <span className="dm-na" role="img" title={text} aria-label={text}>
      <Ban aria-hidden />
    </span>
  );
}

function Chip({ icon, label, value, reason }: { icon: ReactNode; label: string; value: string | null; reason?: string | null | undefined }) {
  return (
    <span className="dm-chip" title={value == null ? undefined : label}>
      {icon}
      <span className="sr-only">{label}</span>
      {value == null ? <Unavailable reason={reason} /> : <b>{value}</b>}
    </span>
  );
}

/** A 0–100% line over the last HISTORY samples, newest at the right. */
function Spark({ points, label, axis = false }: { points: Array<number | null>; label: string; axis?: boolean }) {
  const step = 100 / (HISTORY - 1);
  const offset = HISTORY - points.length;
  const runs: Array<Array<[number, number]>> = [];
  let run: Array<[number, number]> = [];
  points.forEach((value, index) => {
    if (value == null) {
      if (run.length) runs.push(run);
      run = [];
    } else run.push([(offset + index) * step, 40 - (clampLoad(value) / 100) * 38]);
  });
  if (run.length) runs.push(run);
  const line = runs.map(r => `M${r.map(([x, y]) => `${x.toFixed(2)},${y.toFixed(2)}`).join('L')}`).join('');
  const area = runs.filter(r => r.length > 1).map(r => `M${r[0]![0].toFixed(2)},40L${r.map(([x, y]) => `${x.toFixed(2)},${y.toFixed(2)}`).join('L')}L${r[r.length - 1]![0].toFixed(2)},40Z`).join('');
  return (
    <svg className="dm-spark" viewBox="0 0 100 40" preserveAspectRatio="none" role="img" aria-label={label}>
      <path className="dm-spark-grid" d={axis ? 'M0,2H100M0,21H100M0,40H100' : 'M0,21H100'} vectorEffect="non-scaling-stroke" />
      <path className="dm-spark-area" d={area} />
      <path className="dm-spark-line" d={line} vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

function Sensors({ reading }: { reading: TemperatureReadingDto | undefined }) {
  if (!reading?.sensors.length) return null;
  return (
    <details className="dm-sensors">
      <summary>{translate('devices.cPUTemperatureSensors')}{reading.sensors.length})</summary>
      <div>
        {reading.sensors.map((sensor, index) => (
          <span key={`${sensor.label}-${index}`}>
            <span>{sensor.label}</span>
            <b>{sensor.celsius.toFixed(1)} °C</b>
          </span>
        ))}
      </div>
    </details>
  );
}

export function DeviceMonitor() {
  const { locale: i18nLocale } = useI18n();
  const [open, setOpen] = useState(false);
  const [data, setData] = useState<DeviceMetricsDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [forbidden, setForbidden] = useState(false);
  const [now, setNow] = useState(Date.now);
  const [history, setHistory] = useState<CpuSample[]>([]);
  const [coresOpen, setCoresOpen] = useState(false);
  const coresId = useId();
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
          setHistory(current => current.at(-1)?.at === sample.sampledAt ? current : [
            ...current.slice(1 - HISTORY),
            { at: sample.sampledAt, total: sample.cpu.usagePercent, cores: sample.cpu.cores.map(core => core.usagePercent) },
          ]);
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
        if (!disposed && !stopped) timer = setTimeout(poll, open ? OPEN_POLL_MS : CLOSED_POLL_MS);
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
  const stale = Boolean(error) || (age != null && age > staleAfterSeconds(open));
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
          className="device-monitor-dialog"
          meta={data && (
            <span className="dm-meta" data-stale={stale || undefined}>
              <i aria-hidden />
              {translate('devices.secondsAgo', { value1: age ?? 0 })} · {data.platform}
              {data.environment !== 'native' && data.environment !== 'linux' && data.environment !== data.platform ? (
                <abbr
                  title={data.environment === 'wsl'
                    ? translate("devices.wSLEnvironmentCPUAndMemoryDescribeThe")
                    : data.environment === 'container'
                      ? translate("devices.containerEnvironmentSystemCPUAndMemoryWith")
                      : translate("devices.currentSystemCPUMemoryAndAvailableHardware")}
                >{data.environment === 'wsl' ? 'WSL' : data.environment}</abbr>
              ) : null}
              {data.hardwareNotes.length ? (
                <span className="dm-info" role="img" title={data.hardwareNotes.join('\n')} aria-label={data.hardwareNotes.join(' ')}><Info aria-hidden /></span>
              ) : null}
            </span>
          )}
        >
          <div className="device-monitor-details">
            {error && (
              <p role="status" className="dm-notice">
                <TriangleAlert aria-hidden />
                {error}
              </p>
            )}
            {!data && !error && (
              <p role="status" className="dm-notice">{translate("devices.collectingTheFirstCPUSample")}</p>
            )}
            {data && (
              <>
                <section className="dm-row" aria-label={translate("devices.cPUUtilization")} style={load(data.cpu.usagePercent)}>
                  <button type="button" className="dm-head" aria-expanded={coresOpen} aria-controls={coresId} onClick={() => setCoresOpen(value => !value)}>
                    <Cpu className="dm-icon" aria-hidden />
                    <span className="dm-name">CPU</span>
                    <span className="dm-sub" title={data.cpu.model}>{data.cpu.logicalCoreCount} {translate("devices.cores")}</span>
                    <span className="dm-value">{pct(data.cpu.usagePercent)}</span>
                    <ChevronDown className="dm-chevron" aria-hidden />
                  </button>
                  <Meter value={data.cpu.usagePercent} label={translate("devices.cPUUtilization")} />
                  <div className="dm-chips">
                    <Chip
                      icon={<Thermometer aria-hidden />}
                      label={translate("devices.cPUTemperature")}
                      value={data.cpuTemperature?.celsius == null ? null : `${data.cpuTemperature.celsius.toFixed(1)} °C`}
                      reason={data.cpuTemperature ? data.cpuTemperature.reason : translate("devices.updateThisDeviceSSupervisorToCollect")}
                    />
                    <Chip
                      icon={<Zap aria-hidden />}
                      label={translate("devices.cPUPower")}
                      value={data.cpuPower.watts == null ? null : `${data.cpuPower.watts.toFixed(1)} W`}
                      reason={data.cpuPower.reason}
                    />
                  </div>
                  {coresOpen && (
                    <div id={coresId} className="dm-cores-panel">
                      <p className="dm-model">{data.cpu.model}</p>
                      <div className="dm-chart" style={load(data.cpu.usagePercent)}>
                        <Spark points={history.map(sample => sample.total)} label={translate("devices.cPUUtilization")} axis />
                        <span className="dm-axis" aria-hidden>
                          <i style={{ top: `${(2 / 40) * 100}%` }}>100%</i>
                          <i style={{ top: '52.5%' }}>50%</i>
                          <i style={{ top: '100%' }}>0</i>
                        </span>
                        {data.cpu.usagePercent != null && (
                          <span className="dm-now" aria-hidden style={{ top: `${((40 - clampLoad(data.cpu.usagePercent) * 0.38) / 40) * 100}%` }}>
                            {pct(data.cpu.usagePercent)}
                          </span>
                        )}
                      </div>
                      <div className="dm-cores">
                        {data.cpu.cores.map((core, position) => {
                          const name = `${translate("devices.core")} ${core.index + 1}`;
                          const temperature = core.celsius == null ? '' : ` · ${core.celsius.toFixed(0)} °C`;
                          return (
                            <div key={core.index} className="dm-core" style={load(core.usagePercent)} title={`${name} · ${pct(core.usagePercent)}${temperature}`}>
                              <Spark points={history.map(sample => sample.cores[position] ?? null)} label={translate("devices.coreUtilization", { value1: core.index + 1 })} />
                              <span className="dm-core-label">
                                {core.index + 1}
                                {core.celsius != null && (
                                  // 40 °C reads green and 100 °C red, on the same scale as load.
                                  <span className="dm-core-temp" style={load(((core.celsius - 40) / 60) * 100)}>{core.celsius.toFixed(0)}°</span>
                                )}
                              </span>
                              <span className="dm-core-value">{pct(core.usagePercent)}</span>
                            </div>
                          );
                        })}
                      </div>
                      {/* Per-core readings sit on the core charts; older Supervisors list sensors. */}
                      {!data.cpu.cores.some(core => core.celsius != null) && <Sensors reading={data.cpuTemperature} />}
                    </div>
                  )}
                </section>
                <section className="dm-row" aria-label={translate("devices.memoryUtilization")} style={load(data.memory.usagePercent)}>
                  <div className="dm-head">
                    <MemoryStick className="dm-icon" aria-hidden />
                    <span className="dm-name">{translate("devices.memory")}</span>
                    <span className="dm-sub" title={`${translate("devices.available")} ${gib(data.memory.availableBytes)}`}>
                      {gib(data.memory.usedBytes)} / {gib(data.memory.totalBytes)}
                    </span>
                    <span className="dm-value">{pct(data.memory.usagePercent)}</span>
                  </div>
                  <Meter value={data.memory.usagePercent} label={translate("devices.memoryUtilization")} />
                  {data.swap.totalBytes > 0 && (
                    <div className="dm-subrow" style={load(data.swap.usagePercent)}>
                      <ArrowLeftRight className="dm-icon" aria-hidden />
                      <span className="dm-name">{translate("devices.swap")}</span>
                      <span className="dm-sub" title={translate("devices.swapPageFile")}>
                        {gib(data.swap.usedBytes)} / {gib(data.swap.totalBytes)}
                      </span>
                      <Meter value={data.swap.usagePercent} label={translate("devices.swapPageFile")} thin />
                    </div>
                  )}
                </section>
                {data.limits && (data.limits.cpuCores != null || data.limits.memoryTotalBytes != null) && (
                  <section className="dm-row" aria-label={translate("devices.containerLimits")}>
                    <div className="dm-head">
                      <Container className="dm-icon" aria-hidden />
                      <span className="dm-name">{translate("devices.containerLimits")}</span>
                    </div>
                    <div className="dm-chips">
                      {data.limits.cpuCores != null && (
                        <Chip icon={<Cpu aria-hidden />} label={translate("devices.cPUQuota")} value={`${data.limits.cpuCores} ${translate("devices.cores")}`} />
                      )}
                      {data.limits.memoryTotalBytes != null && (
                        <Chip
                          icon={<MemoryStick aria-hidden />}
                          label={translate("devices.memory")}
                          value={`${data.limits.memoryUsedBytes == null ? '—' : gib(data.limits.memoryUsedBytes)} / ${gib(data.limits.memoryTotalBytes)}`}
                        />
                      )}
                    </div>
                  </section>
                )}
                {data.gpus.map((gpu) => (
                  <section key={gpu.id} className="dm-row" aria-label={`GPU ${gpu.name}`} style={load(gpu.usagePercent)}>
                    <div className="dm-head">
                      <Gpu className="dm-icon" aria-hidden />
                      <span className="dm-name" title={gpu.name}>{gpu.name}</span>
                      {gpu.totalMemoryBytes != null && (
                        <span className="dm-sub" title={translate("devices.gPUMemory")}>
                          {gpu.usedMemoryBytes == null ? '—' : gib(gpu.usedMemoryBytes)} / {gib(gpu.totalMemoryBytes)}
                        </span>
                      )}
                      <span className="dm-value">{gpu.usagePercent == null ? <Unavailable /> : pct(gpu.usagePercent)}</span>
                    </div>
                    {gpu.usagePercent != null && <Meter value={gpu.usagePercent} label={`GPU ${gpu.name}`} />}
                    <div className="dm-chips">
                      <Chip
                        icon={<Zap aria-hidden />}
                        label={translate("devices.gPUPower")}
                        value={gpu.power.watts == null ? null : `${gpu.power.watts.toFixed(1)} W`}
                        reason={gpu.power.reason}
                      />
                    </div>
                  </section>
                ))}
              </>
            )}
          </div>
        </FormDialog>
      )}
    </>
  );
}
