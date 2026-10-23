// Devices: every device with its tunnel, protection, version and threads;
// details with the connection history and forwarded traffic the relay
// records, and disconnecting a stuck tunnel.
import { confirmAsync, CopyButton, ErrorMark, MiddleName } from '@pockymoe/thread-ui';
import { DeviceOwnerCell, OwnerLink } from './owners';
import { rangeLabel } from './labels';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ArrowLeftRight, Cpu, Info, Network, Unplug, User } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { adminApi } from './adminApi';
import { ColumnChart, HistoryNote, Sparkline, TimelineStrip, type TimelineSpan } from './charts';
import { formatCount, formatDate, formatDateTime, formatDay, formatDuration, formatRatio, formatRelative, formatTime, parseTime, since } from './format';
import { protocolLabel, reasonLabel, scopeLabel } from './labels';
import type { AdminPageProps } from './RelayAdminConsole';
import type { AdminDevice, AdminDeviceDetail, AdminDeviceFilter, AdminRange } from './types';
import { FilterBar, SelectFilter, TimeFilter, UserFilter, windowParams } from './filters';
import { AdminTable, useFilterValues, useTableSettings, useUrlSort, type TableColumn } from './table';
import {
  Badge,
  Card,
  DetailSection,
  DetailWindow,
  DeviceStateMark,
  errorText,
  Facts,
  LoadFailure,
  Pagination,
  ProtectionMark,
  RowMenu,
  SearchField,
  Skeleton,
  useAdminResource,
  useDebounced,
  type DeviceState,
  type MenuItem,
} from './ui';

const PAGE_SIZE = 25;
const FILTER_KEYS = ['owner', 'state', 'protection', 'version', 'seen', 'created'] as const;
type FilterKey = (typeof FILTER_KEYS)[number];

export function deviceState(device: AdminDevice): DeviceState {
  if (!device.connected) return 'offline';
  return device.unstable ? 'unstable' : 'online';
}

/** Links from before the column filters (`filter=unstable`) still open the right view. */
function legacyFilter(filter: string | null): Partial<Record<FilterKey, string>> {
  switch (filter as AdminDeviceFilter | null) {
    case 'online': return { state: 'connected' };
    case 'offline': return { state: 'offline' };
    case 'unstable': return { state: 'unstable' };
    case 'unprotected': return { protection: 'unprotected' };
    case 'outdated': return { version: 'outdated' };
    default: return {};
  }
}

export function DevicesPage(props: AdminPageProps) {
  useI18n();
  const { refreshKey, onLoaded, onUnauthorized, setParams, selectedId } = props;
  const values = useFilterValues(props, FILTER_KEYS, legacyFilter);
  const urlSort = useUrlSort(props);
  const settings = useTableSettings('devices', { urlSort });
  const [text, setText] = useState('');
  const q = useDebounced(text.trim());
  const [page, setPage] = useState(1);
  // Times are taken when a filter changes, not on every render.
  const query = useMemo(() => ({
    q,
    owner: values.owner,
    state: values.state,
    protection: values.protection,
    version: values.version,
    ...windowParams(values.seen, 'seen'),
    ...windowParams(values.created, 'created'),
    sort: settings.sort?.key,
    dir: settings.sort?.dir,
  }), [q, values, settings.sort?.key, settings.sort?.dir]);
  const queryKey = JSON.stringify(query);
  useEffect(() => setPage(1), [queryKey]);
  const { data, error, loading, reload } = useAdminResource(
    () => adminApi.devices({ ...query, page, pageSize: PAGE_SIZE }),
    `${queryKey}:${page}:${refreshKey}`,
    onUnauthorized,
  );
  useEffect(() => {
    if (data) onLoaded();
  }, [data, onLoaded]);
  const setFilter = (key: FilterKey, value: string | null) => setParams({ [key]: value, filter: null, id: null });
  const active = FILTER_KEYS.filter((key) => values[key]).length;
  const { disconnect, failure } = useDeviceDisconnect(() => {
    reload();
    props.onChanged();
  });
  const menu = (device: AdminDevice): MenuItem[] => [
    { key: 'owner', label: translate('admin.viewOwner'), icon: <User aria-hidden className="h-4 w-4" />, onSelect: () => props.go('users', { id: device.ownerUserId }) },
    { key: 'disconnect', label: translate('admin.disconnect'), icon: <Unplug aria-hidden className="h-4 w-4" />, danger: true, disabled: !device.connected, onSelect: () => void disconnect(device) },
  ];
  const showVersion = data?.items.some((device) => device.version !== undefined) ?? false;
  const showTraffic = data?.items.some((device) => device.traffic7d !== undefined) ?? false;
  const columns: Array<TableColumn<AdminDevice>> = [
    {
      key: 'device',
      label: translate('admin.device'),
      sortable: true,
      hideable: false,
      width: 240,
      minWidth: 160,
      // Long host names keep their end (docs/ui-principles.md §14); the owner
      // goes with the name, which repeats across people.
      render: (device) => (
        <DeviceOwnerCell
          device={{ deviceId: device.id, name: device.name, ownerUserId: device.ownerUserId, ownerUsername: device.ownerUsername, ownerEmail: device.ownerEmail }}
          onOpenDevice={(id) => setParams({ id })}
          onOpenUser={(id) => props.go('users', { id })}
        />
      ),
    },
    { key: 'owner', label: translate('admin.owner'), sortable: true, defaultHidden: true, width: 150, render: (device) => <OwnerLink email={device.ownerEmail} onOpenUser={(id) => props.go('users', { id })} userId={device.ownerUserId} username={device.ownerUsername} /> },
    { key: 'state', label: translate('admin.connection'), sortable: true, width: 190, render: (device) => <ConnectionCell device={device} /> },
    { key: 'lastSeen', label: translate('admin.lastSeenColumn'), sortable: true, firstDir: 'desc', defaultHidden: true, width: 150, render: (device) => <time dateTime={(device.connected ? device.lastHeartbeatAt : device.lastSeenAt) ?? undefined} title={formatDateTime(device.connected ? device.lastHeartbeatAt : device.lastSeenAt)}>{device.connected ? translate('admin.now') : formatRelative(device.lastSeenAt ?? null)}</time> },
    { key: 'protection', label: translate('admin.protection'), sortable: true, width: 140, render: (device) => <ProtectionMark protectedDevice={device.protected} /> },
    ...(showVersion ? [{ key: 'version', label: translate('admin.version'), sortable: true, firstDir: 'desc' as const, width: 130, render: (device: AdminDevice) => <VersionCell device={device} /> }] : []),
    {
      key: 'threads',
      label: translate('admin.threads'),
      sortable: true,
      firstDir: 'desc',
      numeric: true,
      width: 120,
      render: (device) => (
        <span className="admin-two-line">
          <b>{formatCount(device.threadCount)}</b>
          <span>{translate('admin.agentThreadsCount', { count: device.agentThreadCount })}</span>
        </span>
      ),
    },
    { key: 'ports', label: translate('admin.portMappings'), sortable: true, firstDir: 'desc', numeric: true, width: 120, render: (device) => (device.portMappings ? formatCount(device.portMappings) : <span className="admin-muted">—</span>) },
    { key: 'reconnects', label: translate('admin.reconnects1h'), sortable: true, firstDir: 'desc', numeric: true, defaultHidden: true, width: 130, render: (device) => (device.reconnects1h === null || device.reconnects1h === undefined ? <span className="admin-muted">—</span> : formatCount(device.reconnects1h)) },
    ...(showTraffic ? [{
      key: 'traffic',
      label: translate('admin.traffic7d'),
      sortable: true,
      firstDir: 'desc' as const,
      width: 136,
      render: (device: AdminDevice) => <Sparkline label={translate('admin.traffic7dOf', { name: device.name })} values={device.traffic7d ?? []} />,
    }] : []),
    { key: 'created', label: translate('admin.added'), sortable: true, firstDir: 'desc', defaultHidden: true, width: 130, render: (device) => <time dateTime={device.createdAt} title={formatDateTime(device.createdAt)}>{formatDate(device.createdAt)}</time> },
    {
      key: 'menu',
      label: translate('admin.actions'),
      header: <span className="sr-only">{translate('admin.actions')}</span>,
      className: 'is-menu',
      hideable: false,
      fixed: true,
      width: 64,
      render: (device) => (
        <span className="admin-inline">
          <RowMenu items={menu(device)} label={translate('admin.deviceActions', { name: device.name })} />
          {failure?.id === device.id ? <ErrorMark message={failure.message} /> : null}
        </span>
      ),
    },
  ];
  const counts = data?.counts;
  return (
    <div className="admin-page">
      <div className="admin-toolbar">
        <FilterBar active={active} label={translate('admin.filterDevices')} onClear={() => setParams({ ...Object.fromEntries(FILTER_KEYS.map((key) => [key, null])), filter: null, id: null })}>
          <UserFilter label={translate('admin.owner')} onChange={(value) => setFilter('owner', value)} value={values.owner} valueLabel={data?.labels?.owner} />
          <SelectFilter
            label={translate('admin.connection')}
            onChange={(value) => setFilter('state', value)}
            options={[
              { value: 'connected', label: translate('admin.online'), count: counts?.online, icon: <DeviceStateMark showLabel={false} state="online" /> },
              { value: 'unstable', label: translate('admin.unstable'), count: counts?.unstable, icon: <DeviceStateMark showLabel={false} state="unstable" /> },
              { value: 'offline', label: translate('admin.offline'), count: counts?.offline, icon: <DeviceStateMark showLabel={false} state="offline" /> },
            ]}
            value={values.state === 'online' ? 'connected' : values.state}
          />
          <SelectFilter
            label={translate('admin.protection')}
            onChange={(value) => setFilter('protection', value)}
            options={[
              { value: 'protected', label: translate('admin.protected'), count: counts?.protected },
              { value: 'unprotected', label: translate('admin.unprotected'), count: counts?.unprotected },
            ]}
            value={values.protection}
          />
          {showVersion || values.version ? (
            <SelectFilter
              label={translate('admin.version')}
              onChange={(value) => setFilter('version', value)}
              options={[
                { value: 'outdated', label: translate('admin.versionOlder'), count: counts?.outdated },
                { value: 'current', label: translate('admin.versionCurrent') },
                { value: 'unknown', label: translate('admin.versionNotReported'), count: counts?.unknownVersion },
                ...(data?.versions ?? []).map((item) => ({ value: `v:${item.version}`, label: item.version, count: item.devices })),
              ]}
              value={values.version}
            />
          ) : null}
          <TimeFilter label={translate('admin.lastSeenColumn')} never older onChange={(value) => setFilter('seen', value)} value={values.seen} />
          <TimeFilter label={translate('admin.added')} onChange={(value) => setFilter('created', value)} value={values.created} />
        </FilterBar>
        <SearchField label={translate('admin.searchDevices')} onChange={setText} placeholder={translate('admin.searchDevices')} value={text} />
      </div>
      {error && !data ? <LoadFailure message={error} onRetry={reload} /> : null}
      {!data && !error ? <Skeleton lines={8} /> : null}
      {data ? (
        <Card className="admin-table-card">
          <AdminTable
            busy={loading}
            columns={columns}
            empty={translate('admin.noDevicesMatch')}
            id="devices"
            label={translate('admin.navDevices')}
            onOpen={(device) => setParams({ id: device.id })}
            phoneRow={(device) => (
              <>
                <span className="admin-two-line">
                  <b className="admin-device-name" title={device.name}><MiddleName name={device.name} /></b>
                  <span>{[device.ownerUsername, connectionText(device) ?? translate('admin.offline')].join(' · ')}</span>
                </span>
                <span className="admin-list-mark">
                  <ProtectionMark protectedDevice={device.protected} showLabel={false} />
                  <DeviceStateMark showLabel={false} state={deviceState(device)} />
                </span>
              </>
            )}
            rowKey={(device) => device.id}
            rows={data.items}
            selected={selectedId}
            settings={settings}
            total={data.total}
          />
          <Pagination onPage={setPage} page={data.page} pageSize={data.pageSize} total={data.total} />
        </Card>
      ) : null}
      <DeviceDetail id={selectedId} onClose={() => setParams({ id: null })} onDisconnect={disconnect} props={props} />
    </div>
  );
}

/** Disconnecting a tunnel after a confirmation; a failure stays beside the row. */
export function useDeviceDisconnect(onDone: () => void) {
  const [failure, setFailure] = useState<{ id: string; message: string } | null>(null);
  const disconnect = async (device: AdminDevice) => {
    const confirmed = await confirmAsync({
      title: translate('admin.disconnectTitle', { name: device.name }),
      description: translate('admin.disconnectBody'),
      confirmLabel: translate('admin.disconnect'),
    });
    if (!confirmed) return false;
    setFailure(null);
    try {
      await adminApi.disconnectDevice(device.id);
      onDone();
      return true;
    } catch (caught) {
      setFailure({ id: device.id, message: errorText(caught) });
      return false;
    }
  };
  return { disconnect, failure };
}

/** How long it has been connected, or when it was last seen (when recorded). */
function connectionText(device: AdminDevice) {
  if (device.connected) return translate('admin.connectedFor', { time: formatDuration(since(device.connectedAt)) });
  if (device.lastSeenAt) return translate('admin.lastSeen', { time: formatRelative(device.lastSeenAt) });
  return null;
}

function ConnectionCell({ device }: { device: AdminDevice }) {
  const text = connectionText(device);
  return (
    <span className="admin-two-line">
      <DeviceStateMark state={deviceState(device)} />
      {text ? <span>{text}</span> : null}
    </span>
  );
}

function VersionCell({ device }: { device: AdminDevice }) {
  useI18n();
  if (!device.version) return <span className="admin-muted" title={translate('admin.versionUnknownHelp')}>—</span>;
  return (
    <span className="admin-inline">
      <span className="admin-mono">{device.version}</span>
      {device.outdated ? <Badge tone="warning">{translate('admin.old')}</Badge> : null}
    </span>
  );
}

const DETAIL_RANGES: AdminRange[] = ['24h', '7d', '30d'];

export function DeviceDetail({ id, onClose, onDisconnect, props }: { id: string | null; onClose: () => void; onDisconnect: (device: AdminDevice) => Promise<boolean>; props: AdminPageProps }) {
  useI18n();
  // The drawer's own range (tunnels are kept 30 days), in the URL as `drange`.
  const requested = props.params.get('drange') as AdminRange | null;
  const range: AdminRange = requested && DETAIL_RANGES.includes(requested) ? requested : '24h';
  const { data, error, reload } = useAdminResource<AdminDeviceDetail | null>(
    () => (id ? adminApi.device(id, range) : Promise.resolve(null)),
    `${id}:${range}:${props.refreshKey}`,
    props.onUnauthorized,
  );
  const detail = data && data.device.id === id ? data : null;
  const device = detail?.device;
  return (
    <DetailWindow
      actions={
        device ? (
          <>
            <button className="relay-button-secondary" onClick={() => props.go('users', { id: device.ownerUserId })} type="button">
              <User aria-hidden className="mr-2 h-4 w-4" />
              {translate('admin.viewOwner')}
            </button>
            <button className="ui-action-danger admin-action-button" disabled={!device.connected} onClick={() => void onDisconnect(device).then((done) => done && reload())} type="button">
              <Unplug aria-hidden className="mr-2 h-4 w-4" />
              {translate('admin.disconnect')}
            </button>
          </>
        ) : null
      }
      badges={device ? (
        <>
          <DeviceStateMark label={device.connected ? `${translate('admin.online')} · ${formatDuration(since(device.connectedAt))}` : undefined} state={deviceState(device)} />
          <ProtectionMark protectedDevice={device.protected} />
        </>
      ) : null}
      icon={<Cpu aria-hidden className="h-5 w-5" />}
      onClose={() => {
        if (props.params.get('drange')) props.setParams({ drange: null });
        onClose();
      }}
      open={Boolean(id)}
      subtitle={device ? translate('admin.deviceSubtitle', { owner: device.ownerEmail ? `${device.ownerUsername} (${device.ownerEmail})` : device.ownerUsername, date: formatDate(device.createdAt) }) : null}
      title={device?.name ?? translate('admin.loading')}
    >
      {!detail || !device ? (error ? <LoadFailure message={error} onRetry={reload} /> : <Skeleton lines={6} />) : (
        <>
          {detail.connections !== undefined || detail.traffic !== undefined ? (
            <div aria-label={translate('admin.detailRange')} className="admin-range admin-detail-range" role="group">
              {DETAIL_RANGES.map((value) => (
                <button aria-pressed={range === value} key={value} onClick={() => props.setParams({ drange: value === '24h' ? null : value })} type="button">
                  {rangeLabel(value)}
                </button>
              ))}
            </div>
          ) : null}
          {detail.connections !== undefined ? <ConnectionHistory detail={detail} range={range} /> : null}
          {detail.traffic !== undefined ? <Traffic detail={detail} range={range} /> : null}
          <DetailSection icon={<Info aria-hidden className="h-4 w-4" />} title={translate('admin.details')}>
            <Facts
              items={[
                {
                  label: translate('admin.owner'),
                  value: (
                    <span className="admin-inline">
                      <OwnerLink onOpenUser={(user) => props.go('users', { id: user })} userId={device.ownerUserId} username={device.ownerUsername} />
                      {device.ownerEmail ? <span className="admin-muted">{device.ownerEmail}</span> : null}
                    </span>
                  ),
                },
                device.version !== undefined && {
                  label: translate('admin.version'),
                  value: device.version ? (
                    <span className="admin-inline">
                      <span className="admin-mono">{device.version}</span>
                      {device.outdated ? <Badge tone="warning">{translate('admin.old')}</Badge> : null}
                      <span className="admin-muted">{translate('admin.checkedAt', { time: formatRelative(device.versionCheckedAt) })}</span>
                    </span>
                  ) : translate('admin.unknown'),
                },
                { label: translate('admin.lastHeartbeat'), value: device.connected ? formatRelative(device.lastHeartbeatAt) : device.lastSeenAt ? formatRelative(device.lastSeenAt) : '—' },
                { label: translate('admin.threads'), value: translate('admin.threadsDetail', { threads: formatCount(device.threadCount), agents: formatCount(device.agentThreadCount), workspaces: formatCount(device.workspaceCount) }) },
                { label: translate('admin.portMappings'), value: translate('admin.portMappingsDetail', { mappings: formatCount(detail.previews.mappings), sessions: formatCount(detail.previews.sessions) }) },
                { label: translate('admin.upstreams'), value: detail.upstreamProtocols.length ? detail.upstreamProtocols.map(protocolLabel).join(' · ') : '—' },
                {
                  label: translate('admin.navShares'),
                  value: detail.shares.length ? (
                    <ul className="admin-plain-list">
                      {detail.shares.map((share) => (
                        <li key={share.id}>
                          {scopeLabel(share.scope)} →{' '}
                          <OwnerLink onOpenUser={(user) => props.go('users', { id: user })} userId={share.targetUserId} username={share.targetUsername} />
                        </li>
                      ))}
                    </ul>
                  ) : '—',
                },
                { label: translate('admin.protectedSince'), value: device.protected ? (device.protectedAt ? formatDate(device.protectedAt) : translate('admin.protected')) : translate('admin.unprotected') },
                {
                  label: translate('admin.deviceId'),
                  value: (
                    <span className="admin-id">
                      <code>{device.id}</code>
                      <CopyButton label={translate('admin.copyDeviceId')} value={device.id} />
                    </span>
                  ),
                },
              ]}
            />
          </DetailSection>
        </>
      )}
    </DetailWindow>
  );
}

function ConnectionHistory({ detail, range }: { detail: AdminDeviceDetail; range: AdminRange }) {
  useI18n();
  const history = detail.connections;
  if (!history) {
    return (
      <DetailSection icon={<Network aria-hidden className="h-4 w-4" />} title={translate('admin.tunnelHistoryIn', { range: rangeLabel(range) })}>
        <p className="admin-empty">{translate('admin.notRecordedYet')}</p>
      </DetailSection>
    );
  }
  const end = Date.now();
  const start = parseTime(history.since) ?? end - 24 * 3600_000;
  const historyStart = parseTime(history.historyStart);
  const spans: TimelineSpan[] = [];
  if (historyStart && historyStart > start) spans.push({ from: start, to: historyStart, state: 'unknown' });
  for (const span of history.spans) {
    const from = Math.max(start, parseTime(span.from) ?? start);
    const to = Math.min(end, parseTime(span.to) ?? end);
    if (to > from) spans.push({ from, to, state: 'online' });
  }
  // Short connections (under ten minutes) inside an hour of reconnects read as unstable.
  for (const span of spans) {
    if (span.state === 'online' && span.to - span.from < 10 * 60_000 && history.reconnects >= 5) span.state = 'unstable';
  }
  return (
    <DetailSection icon={<Network aria-hidden className="h-4 w-4" />} title={translate('admin.tunnelHistoryIn', { range: rangeLabel(range) })}>
      <div className="admin-timeline-offline-base">
        <TimelineStrip end={end} formatAt={range === '24h' ? formatTime : formatDay} label={translate('admin.tunnelHistoryLabelIn', { uptime: formatRatio(history.uptime, 1), reconnects: history.reconnects, range: rangeLabel(range) })} spans={spans} start={start} />
      </div>
      <ul className="admin-legend">
        <li><span aria-hidden className="admin-swatch admin-series-3" /><span>{translate('admin.online')}</span></li>
        <li><span aria-hidden className="admin-swatch admin-series-4" /><span>{translate('admin.unstable')}</span></li>
        <li><span aria-hidden className="admin-swatch is-offline" /><span>{translate('admin.offline')}</span></li>
      </ul>
      <p className="admin-detail-stats">
        <span>{translate('admin.uptime', { value: formatRatio(history.uptime, 1) })}</span>
        <span>{translate('admin.reconnectsCount', { count: history.reconnects })}</span>
        <span>{translate('admin.longestOffline', { time: formatDuration(history.longestOfflineSeconds) })}</span>
      </p>
      {history.spans.length ? (
        <details className="admin-disclosure">
          <summary>{translate('admin.connectionList')}</summary>
          <ul className="admin-plain-list">
            {history.spans.slice().reverse().slice(0, 20).map((span, index) => (
              <li key={index}>{formatDateTime(span.from)} – {span.to ? formatDateTime(span.to) : translate('admin.now')}{span.reason ? ` · ${reasonLabel(span.reason)}` : ''}</li>
            ))}
          </ul>
        </details>
      ) : null}
      <HistoryNote historyStart={history.historyStart} rangeStart={start} />
    </DetailSection>
  );
}

function Traffic({ detail, range }: { detail: AdminDeviceDetail; range: AdminRange }) {
  useI18n();
  const traffic = detail.traffic;
  const title = translate('admin.trafficIn', { range: rangeLabel(range) });
  if (!traffic) {
    return (
      <DetailSection icon={<ArrowLeftRight aria-hidden className="h-4 w-4" />} title={title}>
        <p className="admin-empty">{translate('admin.notRecordedYet')}</p>
      </DetailSection>
    );
  }
  return (
    <DetailSection icon={<ArrowLeftRight aria-hidden className="h-4 w-4" />} title={title}>
      <ColumnChart
        data={traffic.hours.map((hour) => ({ key: hour.at, label: traffic.resolutionSeconds < 86_400 ? formatTime(hour.at) : formatDay(hour.at), values: { requests: hour.requests } }))}
        height={160}
        label={title}
        series={[{ key: 'requests', label: translate('admin.requests'), slot: 1 }]}
      />
      <p className="admin-chart-note">{traffic.resolutionSeconds < 86_400 ? translate('admin.trafficNote') : translate('admin.trafficNoteDaily')}</p>
      <HistoryNote historyStart={traffic.historyStart} rangeStart={Date.parse(traffic.hours[0]?.at ?? '') || Date.now() - 24 * 3600_000} />
    </DetailSection>
  );
}
