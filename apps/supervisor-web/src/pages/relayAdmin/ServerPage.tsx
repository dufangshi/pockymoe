// Server: whether the relay's host keeps up (docs/relay-admin.md "Server").
// Current values with a status from fixed thresholds (amber at "high", red at
// "critical"), their history over the page's range, and what each means. On
// systems without /proc only the disk and the relay's own load are measured;
// the rest reads "not available". Host figures only, nothing about people.
import { PathChip } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { CircleCheck, CircleDashed, Info, OctagonAlert, TriangleAlert } from 'lucide-react';
import { useEffect, useMemo, type ReactNode } from 'react';
import { adminApi } from './adminApi';
import { BarList, ChartCard, HistoryNote, Legend, LineChart, type LineSeries, type SeriesSlot } from './charts';
import { formatBytes, formatCount, formatDateTime, formatDay, formatDuration, formatRatio, formatRelative, formatTime } from './format';
import { pageWindow, windowLabel } from './ranges';
import type { AdminPageProps } from './RelayAdminConsole';
import type { AdminCheckStatus, AdminSeries, AdminServer, AdminServerCheck } from './types';
import { Card, Facts, LoadFailure, Skeleton, useAdminResource } from './ui';

export function ServerPage(props: AdminPageProps) {
  useI18n();
  const request = useMemo(() => pageWindow(props), [props.range, props.customRange]);
  const { data, error, loading, reload } = useAdminResource(() => adminApi.server(request), `server:${JSON.stringify(request)}:${props.refreshKey}`, props.onUnauthorized);
  const loaded = props.onLoaded;
  useEffect(() => {
    if (data) loaded();
  }, [data, loaded]);
  if (!data) return error ? <LoadFailure message={error} onRetry={reload} /> : <Skeleton lines={8} />;
  return (
    <div aria-busy={loading || undefined} className={`admin-page admin-server${loading ? ' is-busy' : ''}`}>
      {error ? <LoadFailure message={error} onRetry={reload} /> : null}
      <ServerView data={data} />
    </div>
  );
}

const ms = (value: number | null | undefined) => (value === null || value === undefined ? '—' : translate('admin.milliseconds', { value: formatCount(Math.round(value)) }));
const percent = (value: number | null | undefined) => formatRatio(value, 0);
const perSecond = (value: number | null | undefined) => (value === null || value === undefined ? '—' : translate('admin.bytesPerSecond', { value: formatBytes(value) }));

function statusLabel(status: AdminCheckStatus) {
  switch (status) {
    case 'ok': return translate('admin.statusOk');
    case 'warn': return translate('admin.statusHigh');
    case 'critical': return translate('admin.statusCritical');
    default: return translate('admin.notAvailable');
  }
}

/** A status by shape and word, never colour alone: check, triangle, octagon, dashed circle. */
export function HealthMark({ status, showLabel = true }: { status: AdminCheckStatus; showLabel?: boolean }) {
  useI18n();
  const icon = status === 'ok' ? <CircleCheck aria-hidden className="h-4 w-4" /> : status === 'warn' ? <TriangleAlert aria-hidden className="h-4 w-4" /> : status === 'critical' ? <OctagonAlert aria-hidden className="h-4 w-4" /> : <CircleDashed aria-hidden className="h-4 w-4" />;
  return (
    <span className={`admin-health-mark is-${status}`}>
      {icon}
      {showLabel ? <span>{statusLabel(status)}</span> : <span className="sr-only">{statusLabel(status)}</span>}
    </span>
  );
}

function checkOf(data: AdminServer, key: string): AdminServerCheck {
  return data.checks.find((check) => check.key === key) ?? { key, status: 'unknown', value: null, warnAt: 0, criticalAt: 0 };
}

/** The worse of several checks. */
function worst(...checks: AdminServerCheck[]): AdminCheckStatus {
  const rank: Record<AdminCheckStatus, number> = { unknown: 0, ok: 1, warn: 2, critical: 3 };
  return checks.reduce<AdminCheckStatus>((best, check) => (rank[check.status] > rank[best] ? check.status : best), 'unknown');
}

/** A figure tile with its status, value, one detail line and what it means. */
function HealthTile({ label, status, value, detail, help }: { label: string; status: AdminCheckStatus; value: ReactNode; detail?: ReactNode; help: string }) {
  return (
    <div className={`admin-figure admin-health-tile is-${status}`}>
      <dt>
        <span>{label}</span>
        <HealthMark status={status} />
      </dt>
      <dd>
        <span className="admin-figure-value">{value}</span>
      </dd>
      {detail ? <dd className="admin-figure-detail">{detail}</dd> : null}
      <dd className="admin-health-help">{help}</dd>
    </div>
  );
}

const points = (series: AdminSeries | undefined) => (series?.points ?? []).map(([at, value]) => ({ at, value }));
const hasData = (series: AdminSeries | undefined) => Boolean(series?.points.some(([, value]) => value !== null));

function ServerView({ data }: { data: AdminServer }) {
  useI18n();
  const start = Date.parse(data.window.start);
  const end = Date.parse(data.window.end);
  const short = end - start <= 2 * 86_400_000;
  const formatAt = (at: number) => (short ? formatTime(at) : formatDay(at));
  const formatBucket = (at: number) => (short ? formatDateTime(at) : formatDay(at));
  const span = windowLabel(data.window);
  const cpu = checkOf(data, 'cpu');
  const processCpu = checkOf(data, 'processCpu');
  const load = checkOf(data, 'load');
  const memory = checkOf(data, 'memory');
  const swap = checkOf(data, 'swap');
  const disk = checkOf(data, 'disk');
  const files = checkOf(data, 'openFiles');
  const wal = checkOf(data, 'wal');
  const storeBusy = checkOf(data, 'storeBusy');
  const storeWait = checkOf(data, 'storeWait');
  const loopLag = checkOf(data, 'loopLag');
  const queue = checkOf(data, 'tunnelQueue');
  const host = data.host;
  const capacity = data.cpuCapacityCores;
  const limit = data.cgroup?.memoryLimit ?? null;
  const memoryUsed = data.memoryScope === 'cgroup' ? data.cgroup?.memoryBytes ?? null : host.memoryUsed;
  const memoryTotal = data.memoryScope === 'cgroup' ? limit : host.memoryTotal;
  const history = <HistoryNote historyStart={data.historyStart} rangeStart={start} />;
  type Format = (value: number | null) => string;
  const table = (columns: string[], series: Array<AdminSeries | undefined>, format: Format | Format[]) => ({
    columns: [translate('admin.time'), ...columns],
    rows: (series[0]?.points ?? [])
      .map(([at], index) => [formatBucket(at), ...series.map((item, column) => (Array.isArray(format) ? format[column]! : format)(item?.points[index]?.[1] ?? null))])
      .filter((row) => row.slice(1).some((cell) => cell !== '—')),
  });
  const line = (key: string, label: string, slot: SeriesSlot, series: AdminSeries | undefined): LineSeries => ({ key, label, slot, points: points(series) });
  const notAvailable = <p className="admin-empty">{translate('admin.notAvailableHere')}</p>;
  return (
    <>
      {data.platform !== 'linux' ? (
        <p className="admin-card-note admin-server-platform">
          <Info aria-hidden className="h-4 w-4" />
          {translate('admin.serverPlatformNote', { platform: data.platform })}
        </p>
      ) : null}
      <section aria-label={translate('admin.serverNow')} className="admin-card admin-figure-band admin-health-band">
        <dl>
          <HealthTile
            detail={host.load ? translate('admin.loadDetail', { one: host.load[0].toFixed(2), five: host.load[1].toFixed(2), fifteen: host.load[2].toFixed(2), cores: formatCount(host.cores) }) : undefined}
            help={translate('admin.helpCpu')}
            label={translate('admin.cpu')}
            status={worst(cpu, processCpu, load)}
            value={cpu.value === null ? '—' : percent(cpu.value)}
          />
          <HealthTile
            detail={memoryUsed !== null && memoryTotal ? translate(data.memoryScope === 'cgroup' ? 'admin.memoryOfLimit' : 'admin.memoryOfHost', { used: formatBytes(memoryUsed), total: formatBytes(memoryTotal) }) : undefined}
            help={data.memoryScope === 'cgroup' ? translate('admin.helpMemoryCgroup') : translate('admin.helpMemory')}
            label={translate('admin.memory')}
            status={memory.status}
            value={memory.value === null ? '—' : percent(memory.value)}
          />
          <HealthTile
            detail={data.disk.availableBytes !== null ? translate('admin.diskFree', { free: formatBytes(data.disk.availableBytes), total: formatBytes(data.disk.totalBytes) }) : undefined}
            help={translate('admin.helpDisk')}
            label={translate('admin.disk')}
            status={disk.status}
            value={disk.value === null ? '—' : percent(disk.value)}
          />
          <HealthTile
            detail={translate('admin.walSize', { wal: formatBytes(data.disk.walBytes) })}
            help={translate('admin.helpDatabase')}
            label={translate('admin.database')}
            status={wal.status}
            value={formatBytes(data.disk.databaseBytes)}
          />
          <HealthTile
            detail={data.relay.storeLocksPerMinute === null ? translate('admin.storeWaitOnly', { wait: ms(storeWait.value) }) : translate('admin.storeDetail', { wait: ms(storeWait.value), locks: formatCount(data.relay.storeLocksPerMinute) })}
            help={translate('admin.helpStore')}
            label={translate('admin.storeLock')}
            status={worst(storeBusy, storeWait)}
            value={storeBusy.value === null ? '—' : percent(storeBusy.value)}
          />
          <HealthTile
            detail={translate('admin.relayLoadDetail', { tunnels: formatCount(data.relay.deviceTunnels), browsers: formatCount(data.relay.browserSockets), pending: formatCount(data.relay.pendingRequests) })}
            help={translate('admin.helpLoopLag')}
            label={translate('admin.runtimeDelay')}
            status={worst(loopLag, queue)}
            value={ms(loopLag.value)}
          />
        </dl>
      </section>
      <details className="admin-method-note">
        <summary>
          <Info aria-hidden className="h-4 w-4" />
          <span>{translate('admin.thresholdsSummary', { sampled: formatRelative(data.sampledAt) })}</span>
        </summary>
        <ul className="admin-threshold-list">
          {data.checks.map((check) => (
            <li key={check.key}>
              <HealthMark showLabel={false} status={check.status} />
              <b>{checkName(check.key)}</b>
              <span>{checkValue(check)}</span>
              <span className="admin-muted">{translate('admin.thresholdAt', { warn: thresholdText(check, check.warnAt), critical: thresholdText(check, check.criticalAt) })}</span>
            </li>
          ))}
        </ul>
      </details>
      <div className="admin-grid is-halves">
        <ChartCard
          subtitle={translate('admin.cpuSubtitle', { range: span, cores: capacity === null ? '—' : formatCount(capacity) })}
          table={table([translate('admin.relayProcess'), translate('admin.wholeMachine')], [data.series.processCpu, data.series.hostCpu], (value) => percent(value))}
          title={translate('admin.cpu')}
        >
          {hasData(data.series.hostCpu) || hasData(data.series.processCpu) ? (
            <>
              <LineChart
                end={end}
                formatAt={formatAt}
                formatValue={(value) => formatRatio(value, 0)}
                integer={false}
                label={translate('admin.cpu')}
                marks={[{ value: cpu.warnAt, label: translate('admin.markHigh', { value: percent(cpu.warnAt) }), tone: 'warning' }]}
                minMax={1}
                series={[line('process', translate('admin.relayProcess'), 1, data.series.processCpu), line('host', translate('admin.wholeMachine'), 3, data.series.hostCpu)]}
                start={start}
              />
              <Legend items={[{ label: translate('admin.relayProcess'), slot: 1, value: percent(processCpu.value) }, { label: translate('admin.wholeMachine'), slot: 3, value: percent(cpu.value) }]} />
              {history}
            </>
          ) : notAvailable}
        </ChartCard>
        <ChartCard
          subtitle={translate('admin.memorySubtitle', { range: span })}
          table={table([translate('admin.relayProcess'), data.memoryScope === 'cgroup' ? translate('admin.service') : translate('admin.wholeMachine')], [data.series.processRss, data.memoryScope === 'cgroup' ? data.series.cgroupMemory : data.series.hostMemoryUsed], (value) => formatBytes(value))}
          title={translate('admin.memory')}
        >
          {hasData(data.series.processRss) || hasData(data.series.hostMemoryUsed) ? (
            <>
              <LineChart
                end={end}
                formatAt={formatAt}
                formatValue={(value) => formatBytes(value)}
                integer={false}
                label={translate('admin.memory')}
                marks={memoryTotal ? [{ value: memoryTotal, label: data.memoryScope === 'cgroup' ? translate('admin.markLimit', { value: formatBytes(memoryTotal) }) : translate('admin.markTotal', { value: formatBytes(memoryTotal) }), tone: 'limit' }] : []}
                minMax={1024 * 1024}
                series={[
                  line('rss', translate('admin.relayProcess'), 1, data.series.processRss),
                  data.memoryScope === 'cgroup'
                    ? line('cgroup', translate('admin.service'), 3, data.series.cgroupMemory)
                    : line('host', translate('admin.wholeMachine'), 3, data.series.hostMemoryUsed),
                ]}
                start={start}
              />
              <Legend
                items={[
                  { label: translate('admin.relayProcess'), slot: 1, value: formatBytes(data.process.rssBytes) },
                  { label: data.memoryScope === 'cgroup' ? translate('admin.service') : translate('admin.wholeMachine'), slot: 3, value: formatBytes(memoryUsed) },
                ]}
              />
              <p className="admin-chart-note admin-inline">
                {host.swapTotal ? translate('admin.swapNote', { used: formatBytes(host.swapUsed), total: formatBytes(host.swapTotal) }) : translate('admin.noSwap')}
                {swap.status === 'warn' || swap.status === 'critical' ? <HealthMark status={swap.status} /> : null}
              </p>
            </>
          ) : notAvailable}
        </ChartCard>
      </div>
      <div className="admin-grid is-halves">
        <ChartCard
          subtitle={translate('admin.diskSubtitle', { range: span })}
          table={table([translate('admin.diskUsed'), translate('admin.database'), translate('admin.wal')], [data.series.diskUsed, data.series.databaseBytes, data.series.walBytes], (value) => formatBytes(value))}
          title={translate('admin.diskAndDatabase')}
        >
          {hasData(data.series.diskUsed) || hasData(data.series.databaseBytes) ? (
            <>
              {data.disk.totalBytes && hasData(data.series.diskUsed) ? (
                // The disk as a share of its size, with the threshold.
                <LineChart
                  end={end}
                  formatAt={formatAt}
                  formatValue={(value) => formatRatio(value, 0)}
                  height={140}
                  integer={false}
                  label={translate('admin.diskUsed')}
                  marks={[{ value: disk.warnAt, label: translate('admin.markHigh', { value: percent(disk.warnAt) }), tone: 'warning' }]}
                  minMax={1}
                  points={points(data.series.diskUsed).map((point) => ({ at: point.at, value: point.value === null ? null : point.value / data.disk.totalBytes! }))}
                  slot={4}
                  start={start}
                />
              ) : null}
              <LineChart
                end={end}
                formatAt={formatAt}
                formatValue={(value) => formatBytes(value)}
                height={140}
                integer={false}
                label={translate('admin.database')}
                minMax={1024 * 1024}
                series={[line('db', translate('admin.database'), 1, data.series.databaseBytes), line('wal', translate('admin.wal'), 2, data.series.walBytes)]}
                start={start}
              />
              <Legend items={[{ label: translate('admin.diskUsed'), slot: 4, value: `${formatBytes(data.disk.usedBytes)} · ${percent(disk.value)}` }, { label: translate('admin.database'), slot: 1, value: formatBytes(data.disk.databaseBytes) }, { label: translate('admin.wal'), slot: 2, value: formatBytes(data.disk.walBytes) }]} />
              {data.disk.path ? <div className="admin-chart-note"><PathChip label={translate('admin.copyDataDir')} path={data.disk.path} /></div> : null}
            </>
          ) : notAvailable}
        </ChartCard>
        <ChartCard
          subtitle={translate('admin.networkSubtitle', { range: span })}
          table={table([translate('admin.received'), translate('admin.sent')], [data.series.netRx, data.series.netTx], (value) => perSecond(value))}
          title={translate('admin.network')}
        >
          {hasData(data.series.netRx) || hasData(data.series.netTx) ? (
            <>
              <LineChart
                end={end}
                formatAt={formatAt}
                formatValue={(value) => formatBytes(value)}
                integer={false}
                label={translate('admin.network')}
                minMax={1024}
                series={[line('rx', translate('admin.received'), 3, data.series.netRx), line('tx', translate('admin.sent'), 1, data.series.netTx)]}
                start={start}
              />
              <Legend items={[{ label: translate('admin.received'), slot: 3, value: perSecond(host.netRxPerSecond) }, { label: translate('admin.sent'), slot: 1, value: perSecond(host.netTxPerSecond) }]} />
            </>
          ) : notAvailable}
        </ChartCard>
      </div>
      <div className="admin-grid is-halves">
        <ChartCard
          subtitle={translate('admin.relayLoadSubtitle', { range: span })}
          table={table([translate('admin.deviceTunnels'), translate('admin.browserSockets'), translate('admin.pendingRequests')], [data.series.tunnels, data.series.browsers, data.series.pending], (value) => formatCount(value === null ? null : Math.round(value)))}
          title={translate('admin.relayLoad')}
        >
          <LineChart
            end={end}
            formatAt={formatAt}
            formatValue={(value) => formatCount(Math.round(value))}
            label={translate('admin.relayLoad')}
            series={[line('tunnels', translate('admin.deviceTunnels'), 1, data.series.tunnels), line('browsers', translate('admin.browserSockets'), 3, data.series.browsers), line('pending', translate('admin.pendingPeak'), 2, data.series.pending)]}
            start={start}
          />
          <Legend items={[{ label: translate('admin.deviceTunnels'), slot: 1, value: formatCount(data.relay.deviceTunnels) }, { label: translate('admin.browserSockets'), slot: 3, value: formatCount(data.relay.browserSockets) }, { label: translate('admin.pendingPeak'), slot: 2, value: formatCount(data.relay.pendingRequests) }]} />
          {data.relay.tunnelQueue !== null && data.relay.queueCapacity ? (
            <p className="admin-chart-note">{translate('admin.queueNote', { queued: formatCount(data.relay.tunnelQueue), fullest: formatCount(data.relay.tunnelQueueMax), capacity: formatCount(data.relay.queueCapacity) })}</p>
          ) : null}
        </ChartCard>
        <ChartCard
          subtitle={translate('admin.delaysSubtitle', { range: span })}
          table={table([translate('admin.storeBusy'), translate('admin.storeWaitLongest'), translate('admin.runtimeDelay')], [data.series.storeBusy, data.series.storeWaitMax, data.series.loopLag], [percent, ms, ms])}
          title={translate('admin.delays')}
        >
          <LineChart
            end={end}
            formatAt={formatAt}
            formatValue={(value) => ms(value)}
            label={translate('admin.delays')}
            marks={[{ value: loopLag.warnAt, label: translate('admin.markHigh', { value: ms(loopLag.warnAt) }), tone: 'warning' }]}
            series={[line('wait', translate('admin.storeWaitLongest'), 2, data.series.storeWaitMax), line('lag', translate('admin.runtimeDelay'), 4, data.series.loopLag)]}
            start={start}
          />
          <Legend items={[{ label: translate('admin.storeWaitLongest'), slot: 2, value: ms(storeWait.value) }, { label: translate('admin.runtimeDelay'), slot: 4, value: ms(loopLag.value) }]} />
          <LineChart
            end={end}
            formatAt={formatAt}
            formatValue={(value) => formatRatio(value, 0)}
            height={120}
            integer={false}
            label={translate('admin.storeBusy')}
            marks={[{ value: storeBusy.warnAt, label: translate('admin.markHigh', { value: percent(storeBusy.warnAt) }), tone: 'warning' }]}
            minMax={1}
            points={points(data.series.storeBusy)}
            slot={1}
            start={start}
          />
          <p className="admin-chart-note">{translate('admin.storeBusyNote', { busy: percent(storeBusy.value) })}</p>
        </ChartCard>
      </div>
      <div className="admin-grid is-halves">
        <Card subtitle={translate('admin.processSubtitle')} title={translate('admin.relayProcess')}>
          <Facts
            items={[
              { label: translate('admin.pid'), value: <span className="admin-mono">{data.process.pid}</span> },
              { label: translate('admin.uptimeLabel'), value: formatDuration(data.process.uptimeSeconds) },
              { label: translate('admin.residentMemory'), value: formatBytes(data.process.rssBytes) },
              { label: translate('admin.virtualMemory'), value: formatBytes(data.process.vmBytes) },
              { label: translate('admin.processCpu'), value: data.process.cpuCores === null ? '—' : translate('admin.coresUsed', { cores: data.process.cpuCores.toFixed(2) }) },
              { label: translate('admin.threadsCountLabel'), value: formatCount(data.process.threads) },
              {
                label: translate('admin.openFiles'),
                value: (
                  <span className="admin-inline">
                    {data.process.openFilesLimit ? translate('admin.ofLimit', { value: formatCount(data.process.openFiles), limit: formatCount(data.process.openFilesLimit) }) : formatCount(data.process.openFiles)}
                    {files.status === 'warn' || files.status === 'critical' ? <HealthMark status={files.status} /> : null}
                  </span>
                ),
              },
              data.cgroup ? { label: translate('admin.cgroup'), value: <span className="admin-mono admin-truncate" title={data.cgroup.path}>{data.cgroup.path}</span> } : null,
              data.cgroup ? { label: translate('admin.cgroupLimits'), value: translate('admin.cgroupLimitsValue', { memory: data.cgroup.memoryLimit ? formatBytes(data.cgroup.memoryLimit) : translate('admin.none'), cpu: data.cgroup.cpuLimitCores ? translate('admin.coresValue', { cores: data.cgroup.cpuLimitCores.toFixed(1) }) : translate('admin.none') }) } : null,
            ]}
          />
        </Card>
        <Card className="admin-cores" subtitle={translate('admin.coresSubtitle')} title={translate('admin.cores')}>
          {!host.perCore.length && data.platform === 'linux' ? <p className="admin-empty">{translate('admin.afterNextSample')}</p> : host.perCore.length ? (
            <BarList
              formatValue={(value) => formatRatio(value, 0)}
              items={host.perCore.map((value, index) => ({ key: String(index), label: translate('admin.coreN', { n: index }), value, slot: value >= cpu.warnAt ? 4 : 1 }))}
              label={translate('admin.cores')}
              max={1}
            />
          ) : notAvailable}
        </Card>
      </div>
    </>
  );
}

function checkName(key: string) {
  switch (key) {
    case 'cpu': return translate('admin.checkCpu');
    case 'processCpu': return translate('admin.checkProcessCpu');
    case 'load': return translate('admin.checkLoad');
    case 'memory': return translate('admin.checkMemory');
    case 'swap': return translate('admin.checkSwap');
    case 'disk': return translate('admin.checkDisk');
    case 'openFiles': return translate('admin.checkOpenFiles');
    case 'wal': return translate('admin.checkWal');
    case 'storeBusy': return translate('admin.checkStoreBusy');
    case 'storeWait': return translate('admin.checkStoreWait');
    case 'loopLag': return translate('admin.checkLoopLag');
    case 'tunnelQueue': return translate('admin.checkTunnelQueue');
    default: return key;
  }
}

/** Thresholds in the check's own unit: shares as percent, waits in ms, sizes in bytes, load per core. */
function thresholdText(check: AdminServerCheck, value: number) {
  if (check.key === 'wal') return formatBytes(value);
  if (check.key === 'storeWait' || check.key === 'loopLag') return ms(value);
  if (check.key === 'load') return translate('admin.perCoreValue', { value: value.toFixed(1) });
  return formatRatio(value, 0);
}

function checkValue(check: AdminServerCheck) {
  return check.value === null ? translate('admin.notAvailable') : thresholdText(check, check.value);
}
