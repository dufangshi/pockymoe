// Performance: forwarding between browsers and devices in depth over a range
// (24 hours to 90 days, or a custom window): request volume, errors by kind,
// latency percentiles and their distribution, reconnects and how long tunnels
// last, and the devices behind them, each opening the device's details. Every
// chart has a legend, a tooltip and a table view; the relay records counts,
// sizes and times only.
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { Info } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { adminApi, type WindowParams } from './adminApi';
import { DeviceOwnerCell, deviceOwnerText } from './owners';
import { BarList, ChartCard, ColumnChart, HistoryNote, Legend, LineChart, type SeriesSlot } from './charts';
import { DeviceDetail, useDeviceDisconnect } from './DevicesPage';
import { formatBytes, formatCount, formatDateTime, formatDay, formatRatio, formatTime, parseTime } from './format';
import { formatLatency } from './OverviewPage';
import { rangeLabel } from './labels';
import type { AdminPageProps } from './RelayAdminConsole';
import { pageWindow } from './ranges';
import type { AdminDeviceRef, AdminErrorKind, AdminPerformance, AdminPerformanceDevices, AdminReconnectRow, AdminSeries, AdminTrafficRow } from './types';
import { Card, Figure, FigureBand, LoadFailure, Pagination, Skeleton, useAdminResource } from './ui';
import { AdminTable, useTableSettings, type TableColumn } from './table';

const ERROR_KINDS: AdminErrorKind[] = ['timeout', 'busy', 'offline', 'other'];
const ERROR_SLOTS: Record<AdminErrorKind, SeriesSlot> = { timeout: 2, busy: 4, offline: 1, other: 5 };
const DAY = 86_400_000;

export function errorKindLabel(kind: AdminErrorKind) {
  switch (kind) {
    case 'timeout': return translate('admin.errorTimeout');
    case 'busy': return translate('admin.errorBusy');
    case 'offline': return translate('admin.errorOffline');
    case 'other': return translate('admin.errorOther');
  }
}

const points = (series: AdminSeries) => series.points.map(([at, value]) => ({ at, value }));

/** Sums neighbouring buckets so a column chart keeps at most `max` columns. */
function grouped(series: AdminSeries[], max = 48) {
  const length = series[0]?.points.length ?? 0;
  const size = Math.max(1, Math.ceil(length / max));
  const groups: Array<{ at: number; values: Array<number | null> }> = [];
  for (let start = 0; start < length; start += size) {
    groups.push({
      at: series[0]!.points[start]![0],
      values: series.map((item) => {
        const slice = item.points.slice(start, start + size).map(([, value]) => value);
        return slice.every((value) => value === null) ? null : slice.reduce<number>((sum, value) => sum + (value ?? 0), 0);
      }),
    });
  }
  return { groups, stepMs: (series[0]?.resolutionSeconds ?? 60) * 1000 * size };
}

export function PerformancePage(props: AdminPageProps) {
  useI18n();
  const { range, customRange, refreshKey, onLoaded, onUnauthorized, selectedId, setParams } = props;
  const request = useMemo(() => pageWindow({ range, customRange }), [customRange, range]);
  const { data, error, loading, reload } = useAdminResource(() => adminApi.performance(request), `${JSON.stringify(request)}:${refreshKey}`, onUnauthorized);
  useEffect(() => {
    if (data) onLoaded();
  }, [data, onLoaded]);
  const { disconnect } = useDeviceDisconnect(() => {
    reload();
    props.onChanged();
  });
  const detail = <DeviceDetail id={selectedId} onClose={() => setParams({ id: null })} onDisconnect={disconnect} props={props} />;
  if (!data) return <div className="admin-page">{error ? <LoadFailure message={error} onRetry={reload} /> : <Skeleton lines={8} />}{detail}</div>;
  return (
    <div aria-busy={loading || undefined} className={`admin-page admin-performance${loading ? ' is-busy' : ''}`}>
      {error ? <LoadFailure message={error} onRetry={reload} /> : null}
      <PerformanceView data={data} openDevice={(id) => setParams({ id })} props={props} chartWindow={request} />
      {detail}
    </div>
  );
}

function PerformanceView({ data, openDevice, props, chartWindow }: { data: AdminPerformance; openDevice: (id: string) => void; props: AdminPageProps; chartWindow: WindowParams }) {
  useI18n();
  const start = Date.parse(data.start);
  const end = Date.parse(data.end);
  const short = end - start <= 2 * DAY;
  const formatAt = (at: number) => (short ? formatTime(at) : formatDay(at));
  const formatBucket = (at: number) => (data.resolutionSeconds < 86_400 ? formatDateTime(at) : formatDay(at));
  const totals = data.totals;
  const open = data.latency.openAboveMs;
  const failed = ERROR_KINDS.reduce((sum, kind) => sum + (totals.errors[kind] ?? 0), 0);
  const span = data.range === 'custom' ? `${formatDateTime(start)} – ${formatDateTime(end)}` : rangeLabel(data.range);
  const step = translate('admin.everyStep', { step: stepLabel(data.resolutionSeconds) });
  const history = <HistoryNote historyStart={data.historyStart} rangeStart={start} />;
  const ms = (value: number) => formatLatency(value, open);
  const errorColumns = grouped(ERROR_KINDS.map((kind) => data.series.errors[kind]));
  const reconnectColumns = grouped([data.series.reconnects]);
  return (
    <>
      <FigureBand label={translate('admin.performanceFigures')}>
        <Figure
          detail={translate('admin.bytesDetail', { sent: formatBytes(totals.requestBytes), received: formatBytes(totals.responseBytes) })}
          label={translate('admin.requestsIn', { range: span })}
          value={formatCount(totals.requests)}
        />
        <Figure
          detail={ERROR_KINDS.map((kind) => `${errorKindLabel(kind)} ${formatCount(totals.errors[kind] ?? 0)}`).join(' · ')}
          label={translate('admin.errorRate')}
          value={formatRatio(totals.errorRate)}
        />
        <Figure
          detail={translate('admin.percentilesDetail', { p50: ms(totals.p50Ms ?? NaN), p99: ms(totals.p99Ms ?? NaN) })}
          label={translate('admin.latencyP95')}
          value={totals.p95Ms === null ? '—' : ms(totals.p95Ms)}
        />
        <Figure
          detail={translate('admin.connectionsDetail', { open: formatCount(data.connections.open), devices: formatCount(data.connections.topDevicesTotal ?? data.connections.topDevices.length) })}
          label={translate('admin.reconnects')}
          value={formatCount(totals.reconnects)}
        />
      </FigureBand>
      <details className="admin-method-note">
        <summary>
          <Info aria-hidden className="h-4 w-4" />
          <span>{translate('admin.latencyMethodSummary', { open: formatLatency(open) })}</span>
        </summary>
        <p>{translate('admin.latencyMethod', { bounds: data.latency.boundsMs.map((bound) => formatCount(bound)).join(', '), open: formatCount(open) })}</p>
      </details>
      <div className="admin-grid is-halves">
        <ChartCard
          subtitle={step}
          table={{ columns: [translate('admin.time'), translate('admin.requests')], rows: data.series.requests.points.filter(([, value]) => value !== null).map(([at, value]) => [formatBucket(at), value ?? 0]) }}
          title={translate('admin.requestVolume')}
        >
          <LineChart end={end} formatAt={formatAt} label={translate('admin.requestVolume')} points={points(data.series.requests)} start={start} />
          {history}
        </ChartCard>
        <ChartCard
          subtitle={translate('admin.errorRateSubtitle', { failed: formatCount(failed), requests: formatCount(totals.requests) })}
          table={{ columns: [translate('admin.time'), translate('admin.errorRate')], rows: data.series.errorRate.points.filter(([, value]) => value !== null).map(([at, value]) => [formatBucket(at), formatRatio(value)]) }}
          title={translate('admin.errorRate')}
        >
          <LineChart end={end} formatAt={formatAt} formatValue={(value) => formatRatio(value, 1)} integer={false} label={translate('admin.errorRate')} minMax={0.01} points={points(data.series.errorRate)} slot={2} start={start} />
          {history}
        </ChartCard>
      </div>
      <div className="admin-grid is-halves">
        <ChartCard
          subtitle={translate('admin.errorsByKindSubtitle')}
          table={{ columns: [translate('admin.time'), ...ERROR_KINDS.map(errorKindLabel)], rows: errorColumns.groups.map((group) => [formatBucket(group.at), ...group.values.map((value) => value ?? 0)]) }}
          title={translate('admin.errorsByKind')}
        >
          <ColumnChart
            data={errorColumns.groups.map((group) => ({ key: String(group.at), label: formatAt(group.at), values: Object.fromEntries(ERROR_KINDS.map((kind, index) => [kind, group.values[index] ?? 0])) }))}
            height={200}
            label={translate('admin.errorsByKind')}
            series={ERROR_KINDS.map((kind) => ({ key: kind, label: errorKindLabel(kind), slot: ERROR_SLOTS[kind] }))}
          />
          <Legend items={ERROR_KINDS.map((kind) => ({ label: errorKindLabel(kind), slot: ERROR_SLOTS[kind], value: formatCount(totals.errors[kind] ?? 0) }))} />
          <p className="admin-chart-note">{translate('admin.errorKindsNote')}</p>
        </ChartCard>
        <ChartCard
          subtitle={translate('admin.latencySubtitle')}
          table={{
            columns: [translate('admin.time'), 'p50', 'p95', 'p99'],
            rows: data.series.p50.points.map(([at, value], index) => [formatBucket(at), value === null ? '—' : ms(value), ms(data.series.p95.points[index]?.[1] ?? NaN), ms(data.series.p99.points[index]?.[1] ?? NaN)]).filter((row) => row[1] !== '—'),
          }}
          title={translate('admin.latencyPercentiles')}
        >
          <LineChart
            end={end}
            formatAt={formatAt}
            formatValue={(value) => formatLatency(value)}
            label={translate('admin.latencyPercentiles')}
            series={[
              { key: 'p50', label: 'p50', slot: 3, points: points(data.series.p50) },
              { key: 'p95', label: 'p95', slot: 1, points: points(data.series.p95) },
              { key: 'p99', label: 'p99', slot: 2, points: points(data.series.p99) },
            ]}
            start={start}
          />
          <Legend items={[{ label: 'p50', slot: 3, value: ms(totals.p50Ms ?? NaN) }, { label: 'p95', slot: 1, value: ms(totals.p95Ms ?? NaN) }, { label: 'p99', slot: 2, value: ms(totals.p99Ms ?? NaN) }]} />
        </ChartCard>
      </div>
      <div className="admin-grid is-halves">
        <ChartCard
          subtitle={translate('admin.latencyDistributionSubtitle')}
          table={{ columns: [translate('admin.latencyBucket'), translate('admin.requests')], rows: data.latencyHistogram.map((bucket) => [bucketLabel(bucket), bucket.count]) }}
          title={translate('admin.latencyDistribution')}
        >
          <ColumnChart
            data={data.latencyHistogram.map((bucket) => ({ key: String(bucket.toMs ?? 'open'), label: bucket.toMs === null ? `>${compactMs(bucket.fromMs)}` : compactMs(bucket.toMs), values: { count: bucket.count } }))}
            height={200}
            label={translate('admin.latencyDistribution')}
            series={[{ key: 'count', label: translate('admin.requests'), slot: 1 }]}
          />
          <p className="admin-chart-note">{translate('admin.latencyBucketNote')}</p>
        </ChartCard>
        <ChartCard
          subtitle={translate('admin.perStep', { step: stepLabel(data.resolutionSeconds) })}
          table={{
            columns: [translate('admin.time'), translate('admin.bytesSent'), translate('admin.bytesReceived')],
            rows: data.series.requestBytes.points.filter(([, value]) => value !== null).map(([at, value], index) => [formatBucket(at), formatBytes(value), formatBytes(data.series.responseBytes.points[index]?.[1] ?? null)]),
          }}
          title={translate('admin.trafficVolume')}
        >
          <LineChart
            end={end}
            formatAt={formatAt}
            formatValue={(value) => formatBytes(value)}
            label={translate('admin.trafficVolume')}
            series={[
              { key: 'sent', label: translate('admin.bytesSent'), slot: 1, points: points(data.series.requestBytes) },
              { key: 'received', label: translate('admin.bytesReceived'), slot: 3, points: points(data.series.responseBytes) },
            ]}
            start={start}
          />
          <Legend items={[{ label: translate('admin.bytesSent'), slot: 1, value: formatBytes(totals.requestBytes) }, { label: translate('admin.bytesReceived'), slot: 3, value: formatBytes(totals.responseBytes) }]} />
          <p className="admin-chart-note">{translate('admin.bytesNote')}</p>
        </ChartCard>
      </div>
      <div className="admin-grid is-halves">
        <ChartCard
          subtitle={translate('admin.reconnectsSubtitle')}
          table={{ columns: [translate('admin.time'), translate('admin.reconnects')], rows: reconnectColumns.groups.map((group) => [formatBucket(group.at), group.values[0] ?? 0]) }}
          title={translate('admin.reconnectsOverTime')}
        >
          <ColumnChart
            data={reconnectColumns.groups.map((group) => ({ key: String(group.at), label: formatAt(group.at), values: { count: group.values[0] ?? 0 } }))}
            height={180}
            label={translate('admin.reconnectsOverTime')}
            series={[{ key: 'count', label: translate('admin.reconnects'), slot: 4 }]}
          />
          {history}
        </ChartCard>
        <ChartCard
          subtitle={translate('admin.durationsSubtitle', { open: formatCount(data.connections.open) })}
          table={{ columns: [translate('admin.connectionLength'), translate('admin.connections')], rows: data.connections.durations.map((bucket) => [durationLabel(bucket.key), bucket.count]) }}
          title={translate('admin.connectionDurations')}
        >
          <BarList
            items={data.connections.durations.map((bucket) => ({ key: bucket.key, label: durationLabel(bucket.key), value: bucket.count, slot: bucket.key === 'lt1m' || bucket.key === 'lt10m' ? 4 : 3 }))}
            label={translate('admin.connectionDurations')}
          />
          <HistoryNote historyStart={data.connections.historyStart} rangeStart={start} />
        </ChartCard>
      </div>
      <UnstableDevices openDevice={openDevice} props={props} chartWindow={chartWindow} />
      <DeviceTraffic historyStart={parseTime(data.historyStart)} openDevice={openDevice} props={props} start={start} chartWindow={chartWindow} />
    </>
  );
}

function stepLabel(seconds: number) {
  if (seconds < 3600) return translate('admin.stepMinutes', { count: Math.round(seconds / 60) });
  if (seconds < 86_400) return translate('admin.stepHours', { count: Math.round(seconds / 3600) });
  return translate('admin.stepDays', { count: Math.round(seconds / 86_400) });
}

function compactMs(value: number) {
  return value >= 1000 ? `${formatCount(value / 1000)}s` : `${formatCount(value)}`;
}

function bucketLabel(bucket: { fromMs: number; toMs: number | null }) {
  return bucket.toMs === null
    ? translate('admin.bucketAbove', { from: formatLatency(bucket.fromMs) })
    : translate('admin.bucketRange', { from: formatLatency(bucket.fromMs), to: formatLatency(bucket.toMs) });
}

function durationLabel(key: AdminPerformance['connections']['durations'][number]['key']) {
  switch (key) {
    case 'lt1m': return translate('admin.durationLt1m');
    case 'lt10m': return translate('admin.durationLt10m');
    case 'lt1h': return translate('admin.durationLt1h');
    case 'lt6h': return translate('admin.durationLt6h');
    case 'lt24h': return translate('admin.durationLt24h');
    case 'gte24h': return translate('admin.durationGte24h');
  }
}

const text = (value: string | null | undefined) => (value ?? '').toLowerCase();
const DEVICE_PAGE = 10;

/** The device and its owner; the name opens the device, the owner the person. */
function deviceColumn<T extends AdminDeviceRef>(openDevice: (id: string) => void, go: AdminPageProps['go']): TableColumn<T> {
  return {
    key: 'device',
    label: translate('admin.deviceAndOwner'),
    hideable: false,
    sortable: true,
    width: 240,
    render: (row) => <DeviceOwnerCell device={row} onOpenDevice={openDevice} onOpenUser={(id) => go('users', { id })} />,
  };
}

/** One server-paged, server-sorted list of devices for the window. */
function useDevicePage<T>(id: string, chartWindow: WindowParams, refreshKey: number, onUnauthorized: () => void, load: (params: WindowParams & Record<string, string | number | undefined>) => Promise<AdminPerformanceDevices<T>>, defaultSort: { key: string; dir: 'asc' | 'desc' }) {
  const settings = useTableSettings(id, { defaultSort });
  const [page, setPage] = useState(1);
  const sortKey = `${settings.sort?.key ?? ''}:${settings.sort?.dir ?? ''}`;
  const windowKey = JSON.stringify(chartWindow);
  useEffect(() => setPage(1), [sortKey, windowKey]);
  const resource = useAdminResource(
    () => load({ ...chartWindow, page, pageSize: DEVICE_PAGE, sort: settings.sort?.key, dir: settings.sort?.dir }),
    `${id}:${windowKey}:${page}:${sortKey}:${refreshKey}`,
    onUnauthorized,
  );
  return { settings, page, setPage, ...resource };
}

function UnstableDevices({ chartWindow, openDevice, props }: { chartWindow: WindowParams; openDevice: (id: string) => void; props: AdminPageProps }) {
  useI18n();
  const { settings, setPage, data, error, loading, reload } = useDevicePage('performance-unstable', chartWindow, props.refreshKey, props.onUnauthorized, adminApi.reconnectDevices, { key: 'connects', dir: 'desc' });
  const columns: Array<TableColumn<AdminReconnectRow>> = [
    deviceColumn<AdminReconnectRow>(openDevice, props.go),
    { key: 'connects', label: translate('admin.reconnects'), sortable: true, numeric: true, firstDir: 'desc', width: 180, render: (row) => formatCount(row.connects) },
    { key: 'short', label: translate('admin.shortConnections'), sortable: true, numeric: true, firstDir: 'desc', width: 180, render: (row) => formatCount(row.shortConnections) },
  ];
  return (
    <Card className="admin-table-card is-headed" subtitle={translate('admin.unstableDevicesSubtitle')} title={translate('admin.unstableDevicesTitle')}>
      {error && !data ? <LoadFailure message={error} onRetry={reload} /> : null}
      {data ? (
        <>
          <AdminTable
            busy={loading}
            columns={columns}
            empty={translate('admin.noReconnects')}
            id="performance-unstable"
            label={translate('admin.unstableDevicesTitle')}
            onOpen={(row) => openDevice(row.deviceId)}
            phoneRow={(row) => (
              <span className="admin-two-line">
                <b>{deviceOwnerText(row)}</b>
                <span>{translate('admin.reconnectsRow', { connects: formatCount(row.connects), short: formatCount(row.shortConnections) })}</span>
              </span>
            )}
            rowKey={(row) => row.deviceId}
            rows={data.items}
            settings={settings}
            total={data.total}
          />
          <Pagination onPage={setPage} page={data.page} pageSize={data.pageSize} total={data.total} />
        </>
      ) : !error ? <Skeleton lines={4} /> : null}
    </Card>
  );
}

function DeviceTraffic({ chartWindow, openDevice, props, historyStart, start }: { chartWindow: WindowParams; openDevice: (id: string) => void; props: AdminPageProps; historyStart: number | null; start: number }) {
  useI18n();
  const { settings, setPage, data, error, loading, reload } = useDevicePage('performance-traffic', chartWindow, props.refreshKey, props.onUnauthorized, adminApi.trafficDevices, { key: 'requests', dir: 'desc' });
  const rows = data?.items ?? [];
  const busiest = Math.max(1, ...rows.map((row) => row.requests));
  // Shares are of all devices' requests in the window, not of this page.
  const total = data?.requests ?? rows.reduce((sum, row) => sum + row.requests, 0);
  const rate = (row: AdminTrafficRow) => (row.requests ? row.errors / row.requests : 0);
  const columns: Array<TableColumn<AdminTrafficRow>> = [
    deviceColumn<AdminTrafficRow>(openDevice, props.go),
    {
      key: 'requests',
      label: translate('admin.requests'),
      sortable: true,
      numeric: true,
      firstDir: 'desc',
      width: 200,
      render: (row) => (
        <span className="admin-share-cell">
          <span aria-hidden className="admin-share-bar"><span className="admin-series-1" style={{ width: `${Math.max(2, (row.requests / busiest) * 100)}%` }} /></span>
          <b>{formatCount(row.requests)}</b>
          <span className="admin-muted">{total ? formatRatio(row.requests / total, 0) : ''}</span>
        </span>
      ),
    },
    { key: 'errors', label: translate('admin.errors'), sortable: true, numeric: true, firstDir: 'desc', width: 110, render: (row) => formatCount(row.errors) },
    { key: 'rate', label: translate('admin.errorRate'), sortable: true, numeric: true, firstDir: 'desc', width: 120, render: (row) => formatRatio(rate(row), 1) },
    { key: 'sent', label: translate('admin.bytesSent'), sortable: true, numeric: true, firstDir: 'desc', width: 130, render: (row) => formatBytes(row.requestBytes) },
    { key: 'received', label: translate('admin.bytesReceived'), sortable: true, numeric: true, firstDir: 'desc', width: 140, render: (row) => formatBytes(row.responseBytes) },
  ];
  const daily = (data?.resolutionSeconds ?? 3600) >= 86_400;
  return (
    <Card className="admin-table-card is-headed" subtitle={daily ? translate('admin.deviceTrafficSubtitleDaily') : translate('admin.deviceTrafficSubtitle')} title={translate('admin.deviceTraffic')}>
      {error && !data ? <LoadFailure message={error} onRetry={reload} /> : null}
      {data ? (
        <>
          <AdminTable
            busy={loading}
            columns={columns}
            empty={translate('admin.noTraffic')}
            id="performance-traffic"
            label={translate('admin.deviceTraffic')}
            onOpen={(row) => openDevice(row.deviceId)}
            phoneRow={(row) => (
              <span className="admin-two-line">
                <b>{deviceOwnerText(row)}</b>
                <span>{translate('admin.trafficRow', { requests: formatCount(row.requests), errors: formatCount(row.errors), received: formatBytes(row.responseBytes) })}</span>
              </span>
            )}
            rowKey={(row) => row.deviceId}
            rows={rows}
            settings={settings}
            total={data.total}
          />
          <Pagination onPage={setPage} page={data.page} pageSize={data.pageSize} total={data.total} />
        </>
      ) : !error ? <Skeleton lines={4} /> : null}
      {historyStart && historyStart > start ? <div className="admin-card-pad"><HistoryNote historyStart={historyStart} rangeStart={start} /></div> : null}
    </Card>
  );
}
