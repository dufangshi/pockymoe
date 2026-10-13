import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { type CSSProperties, type ReactNode, useEffect, useId, useRef, useState } from 'react';
import { ArrowLeftRight, Ban, ChevronDown, ChevronRight, Container, Cpu, Gpu, Info, MemoryStick, Settings, Thermometer, TriangleAlert, Zap } from 'lucide-react';
import type { DeviceMetricsDto, HarnessProcessDto, HarnessProcessesDto, LoadedThreadDto, TemperatureReadingDto } from '@pockymoe/shared';
import { ApiError, fetchHarnessProcesses, releaseThread, request } from '../lib/api';
import { onHarnessRuntimeChange } from '../lib/harnessRuntimeEvents';
import { useAppShellNav } from './AppShellNavContext';
import { ConfirmDialog } from './ConfirmDialog';
import { FormDialog } from './FormDialog';
import { harnessState, memory, METRIC_LABEL, ROLE_LABEL } from './harnessResourceModel';
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
/** The expanded memory row refreshes loaded harnesses this often. */
const PROCESS_POLL_MS = 3_000;
function ProcessList({ processes }: { processes: HarnessProcessDto[] }) {
  return (
    <ul className="dm-procs">
      {processes.map(process => (
        <li key={process.pid} title={`PID ${process.pid}`}>
          <span className="dm-proc-role">{ROLE_LABEL[process.role]?.() ?? process.role}</span>
          <span className="dm-proc-name">{process.name}</span>
          <span className="dm-proc-bytes">{memory(process.bytes)}</span>
        </li>
      ))}
    </ul>
  );
}

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
  const shellNav = useAppShellNav();
  const [memoryOpen, setMemoryOpen] = useState(false);
  const memoryId = useId();
  const [processes, setProcesses] = useState<HarnessProcessesDto | null>(null);
  const [processesError, setProcessesError] = useState<string | null>(null);
  // Older Supervisors have no process breakdown; their memory row stays a plain reading.
  const [processesUnsupported, setProcessesUnsupported] = useState(false);
  const [threadsOpen, setThreadsOpen] = useState<ReadonlySet<string>>(new Set());
  const [ending, setEnding] = useState<string | null>(null);
  const [endError, setEndError] = useState<string | null>(null);
  const [confirmEnd, setConfirmEnd] = useState<LoadedThreadDto | null>(null);
  const refreshProcesses = useRef<() => void>(() => {});
  const processesShown = open && memoryOpen && !processesUnsupported;
  useEffect(() => {
    if (!processesShown) return;
    let disposed = false;
    let busy = false;
    let again = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let controller: AbortController | null = null;
    const poll = async () => {
      if (disposed) return;
      if (busy) {
        again = true;
        return;
      }
      clearTimeout(timer);
      if (document.hidden) {
        timer = setTimeout(poll, PROCESS_POLL_MS);
        return;
      }
      busy = true;
      controller = new AbortController();
      const timeout = setTimeout(() => controller?.abort(), 10_000);
      try {
        const sample = await fetchHarnessProcesses(controller.signal);
        if (!disposed) {
          setProcesses(sample);
          setProcessesError(null);
        }
      } catch (cause) {
        if (!disposed) {
          if (cause instanceof ApiError && cause.statusCode === 404) {
            setProcessesUnsupported(true);
            setMemoryOpen(false);
          } else setProcessesError(cause instanceof Error ? cause.message : String(cause));
        }
      } finally {
        clearTimeout(timeout);
        busy = false;
        if (!disposed) {
          if (again) {
            again = false;
            void poll();
          } else timer = setTimeout(poll, PROCESS_POLL_MS);
        }
      }
    };
    refreshProcesses.current = () => void poll();
    void poll();
    const stopListening = onHarnessRuntimeChange(() => void poll());
    return () => {
      disposed = true;
      clearTimeout(timer);
      controller?.abort();
      stopListening();
      refreshProcesses.current = () => {};
    };
  }, [processesShown]);
  const endThread = async (thread: LoadedThreadDto, interrupt: boolean) => {
    setEnding(thread.threadId);
    setEndError(null);
    try {
      await releaseThread(thread.threadId, interrupt ? { interrupt: true } : {});
      setConfirmEnd(null);
      refreshProcesses.current();
    } catch (cause) {
      // The thread started a turn since the last sample: confirm before interrupting.
      if (!interrupt && cause instanceof ApiError && cause.statusCode === 409) setConfirmEnd(thread);
      else setEndError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setEnding(null);
    }
  };
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
  const gpu = data?.gpus.find(gpu => gpu.usagePercent != null ||
    (gpu.usedMemoryBytes != null && gpu.totalMemoryBytes != null && gpu.totalMemoryBytes > 0));
  const vram = gpu?.usedMemoryBytes != null && gpu.totalMemoryBytes != null && gpu.totalMemoryBytes > 0
    ? 100 * gpu.usedMemoryBytes / gpu.totalMemoryBytes : null;
  const reading = (icon: ReactNode, value: number | null | undefined, title: string) => (
    <span className="dm-summary-reading" title={title} aria-label={`${title}: ${pct(stale ? null : value)}`}>
      {icon}<b style={load(stale ? null : value)}>{pct(stale ? null : value)}</b>
    </span>
  );
  const capacity = (used: number | null | undefined, total: number | null | undefined, value: number | null | undefined, title: string) => {
    const text = stale || used == null || total == null || total <= 0
      ? '—' : `${(used / 2 ** 30).toFixed(1)}/${(total / 2 ** 30).toFixed(1)}`;
    return <span className="dm-summary-reading dm-summary-capacity" title={`${title}: ${text} GiB`} aria-label={`${title}: ${text} GiB`}>
      <MemoryStick aria-hidden style={{ width: 8, height: 8 }} /><b style={load(stale ? null : value)}>{text}{text !== '—' && <small>G</small>}</b>
    </span>;
  };
  return (
    <>
      <button
        type="button"
        className="device-monitor-button"
        aria-label={translate("devices.deviceMonitor")}
        aria-expanded={open}
        data-stale={stale || undefined}
        data-gpu={gpu ? true : undefined}
        title={error ?? translate("devices.cPUMemoryAndHardwareSensors")}
        onClick={() => setOpen(true)}
      >
        <span className="dm-summary-row">
          {reading(<Cpu aria-hidden style={{ width: 8, height: 8 }} />, data?.cpu.usagePercent, 'CPU')}
          <span className="dm-summary-separator" aria-hidden>·</span>
          {capacity(data?.memory.usedBytes, data?.memory.totalBytes, data?.memory.usagePercent, 'DRAM')}
        </span>
        {gpu && (
          <span className="dm-summary-row">
            {reading(<Gpu aria-hidden style={{ width: 8, height: 8 }} />, gpu.usagePercent, `GPU · ${gpu.name}`)}
            <span className="dm-summary-separator" aria-hidden>·</span>
            {capacity(gpu.usedMemoryBytes, gpu.totalMemoryBytes, vram, 'VRAM')}
          </span>
        )}
      </button>
      {open && !confirmEnd && (
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
                  {(() => {
                    const head = (
                      <>
                        <MemoryStick className="dm-icon" aria-hidden />
                        <span className="dm-name">{translate("devices.memory")}</span>
                        <span className="dm-sub" title={`${translate("devices.available")} ${gib(data.memory.availableBytes)}`}>
                          {gib(data.memory.usedBytes)} / {gib(data.memory.totalBytes)}
                        </span>
                        <span className="dm-value">{pct(data.memory.usagePercent)}</span>
                      </>
                    );
                    return processesUnsupported ? <div className="dm-head">{head}</div> : (
                      <button type="button" className="dm-head" aria-expanded={memoryOpen} aria-controls={memoryId} onClick={() => setMemoryOpen(value => !value)}>
                        {head}
                        <ChevronDown className="dm-chevron" aria-hidden />
                      </button>
                    );
                  })()}
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
                  {processesShown && (
                    <div id={memoryId} className="dm-harness" aria-label={translate('devices.harnessMemory')}>
                      <div className="dm-harness-total">
                        <span>
                          <b>{translate('devices.supervisorTotal', { value1: memory(processes?.totalBytes ?? null) })}</b>
                          {processes && <span className="dm-harness-sum"> · {translate('devices.harnessTotal', { value1: memory(processes.harnessBytes) })}</span>}
                        </span>
                        {processes && (
                          <span className="dm-info" role="img" title={METRIC_LABEL[processes.metric]?.() ?? processes.metric} aria-label={METRIC_LABEL[processes.metric]?.() ?? processes.metric}>
                            <Info aria-hidden />
                          </span>
                        )}
                        {shellNav?.openSettingsSection && (
                          <button type="button" className="dm-harness-settings" title={translate('devices.harnessResourceSettings')} aria-label={translate('devices.harnessResourceSettings')}
                            onClick={() => { setOpen(false); shellNav.openSettingsSection?.('resources'); }}>
                            <Settings aria-hidden />
                          </button>
                        )}
                      </div>
                      {processes && (processes.overThreadLimit || processes.overMemoryLimit) && (
                        <p role="status" className="dm-notice">
                          <TriangleAlert aria-hidden />
                          {[processes.overThreadLimit && translate('devices.overThreadLimit'), processes.overMemoryLimit && translate('devices.overMemoryLimit')].filter(Boolean).join(' · ')}
                        </p>
                      )}
                      {(processesError || endError) && (
                        <p role="alert" className="dm-notice"><TriangleAlert aria-hidden />{endError ?? processesError}</p>
                      )}
                      {processes && !processes.threads.length && <p className="dm-harness-empty">{translate('devices.harnessNone')}</p>}
                      {processes && processes.threads.length > 0 && (
                        <ul className="dm-harness-threads">
                          {processes.threads.map(thread => {
                            const expanded = threadsOpen.has(thread.threadId);
                            const listId = `${memoryId}-${thread.threadId}`;
                            const shared = thread.sharesProcessWith.length;
                            return (
                              <li key={thread.threadId} data-busy={thread.busy.length ? true : undefined}>
                                <div className="dm-harness-row">
                                  <button type="button" className="dm-harness-toggle" aria-expanded={expanded} aria-controls={listId}
                                    aria-label={translate('devices.harnessProcesses', { value1: thread.title })}
                                    onClick={() => setThreadsOpen(current => {
                                      const next = new Set(current);
                                      if (!next.delete(thread.threadId)) next.add(thread.threadId);
                                      return next;
                                    })}>
                                    <ChevronRight className="dm-harness-chevron" aria-hidden />
                                    <span className="dm-harness-kind">{thread.agentId ?? thread.provider}</span>
                                    <span className="dm-harness-title" title={thread.title}>{thread.title}</span>
                                  </button>
                                  <span className="dm-harness-state" title={shared ? translate('devices.harnessShared', { count: shared }) : undefined}>
                                    {harnessState(thread, now)}{shared ? ' ⇄' : ''}
                                  </span>
                                  <span className="dm-harness-bytes">{memory(thread.bytes)}</span>
                                  <button type="button" className="dm-harness-end" disabled={ending != null}
                                    aria-label={translate('devices.harnessEndLabel', { value1: thread.title })}
                                    onClick={() => thread.busy.length ? setConfirmEnd(thread) : void endThread(thread, false)}>
                                    {ending === thread.threadId ? translate('devices.harnessEnding') : translate('devices.harnessEnd')}
                                  </button>
                                </div>
                                {expanded && <div id={listId}><ProcessList processes={thread.processes} /></div>}
                              </li>
                            );
                          })}
                        </ul>
                      )}
                      {processes && processes.other.length > 0 && (
                        <details className="dm-harness-other">
                          <summary>{translate('devices.harnessOtherProcesses')}</summary>
                          <ProcessList processes={processes.other} />
                        </details>
                      )}
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
      <ConfirmDialog
        open={confirmEnd != null}
        title={translate('devices.harnessEndRunningTitle')}
        description={translate('devices.harnessEndRunningDescription', { value1: confirmEnd?.title ?? '' })}
        confirmLabel={translate('devices.harnessEnd')}
        busyLabel={translate('devices.harnessEnding')}
        busy={ending != null}
        error={endError}
        onCancel={() => { setConfirmEnd(null); setEndError(null); }}
        onConfirm={() => confirmEnd ? endThread(confirmEnd, true) : undefined}
      />
    </>
  );
}
