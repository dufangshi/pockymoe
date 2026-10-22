// Devices: every device with its tunnel, protection, version and threads;
// details with the connection history and forwarded traffic the relay
// records, and disconnecting a stuck tunnel.
import { confirmAsync, CopyButton, ErrorMark, MiddleName } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ArrowLeftRight, Cpu, Info, Network, Unplug, User } from 'lucide-react';
import { useEffect, useState } from 'react';
import { adminApi } from './adminApi';
import { ColumnChart, HistoryNote, Sparkline, TimelineStrip, type TimelineSpan } from './charts';
import { formatCount, formatDate, formatDateTime, formatDuration, formatRatio, formatRelative, formatTime, parseTime, since } from './format';
import { protocolLabel, reasonLabel, scopeLabel } from './labels';
import type { AdminPageProps } from './RelayAdminConsole';
import type { AdminDevice, AdminDeviceDetail, AdminDeviceFilter } from './types';
import {
  Badge,
  Card,
  DataTable,
  DetailSection,
  DetailWindow,
  DeviceStateMark,
  errorText,
  Facts,
  FilterChips,
  LoadFailure,
  Pagination,
  ProtectionMark,
  RowMenu,
  RowOpener,
  SearchField,
  Skeleton,
  useAdminResource,
  useDebounced,
  type Column,
  type DeviceState,
  type MenuItem,
} from './ui';

const PAGE_SIZE = 25;
const FILTERS: AdminDeviceFilter[] = ['all', 'online', 'offline', 'unstable', 'unprotected', 'outdated'];

export function deviceState(device: AdminDevice): DeviceState {
  if (!device.connected) return 'offline';
  return device.unstable ? 'unstable' : 'online';
}

function filterLabel(filter: AdminDeviceFilter) {
  switch (filter) {
    case 'all': return translate('admin.filterAll');
    case 'online': return translate('admin.online');
    case 'offline': return translate('admin.offline');
    case 'unstable': return translate('admin.unstable');
    case 'unprotected': return translate('admin.unprotected');
    case 'outdated': return translate('admin.outdated');
  }
}

export function DevicesPage(props: AdminPageProps) {
  useI18n();
  const { refreshKey, onLoaded, onUnauthorized, setParams, selectedId } = props;
  const filter = (FILTERS.includes(props.filter as AdminDeviceFilter) ? props.filter : 'all') as AdminDeviceFilter;
  const [text, setText] = useState('');
  const q = useDebounced(text.trim());
  const [page, setPage] = useState(1);
  useEffect(() => setPage(1), [filter, q]);
  const { data, error, loading, reload } = useAdminResource(
    () => adminApi.devices({ filter, q, page, pageSize: PAGE_SIZE }),
    `${filter}:${q}:${page}:${refreshKey}`,
    onUnauthorized,
  );
  useEffect(() => {
    if (data) onLoaded();
  }, [data, onLoaded]);
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
      reload();
      props.onChanged();
      return true;
    } catch (caught) {
      setFailure({ id: device.id, message: errorText(caught) });
      return false;
    }
  };
  const menu = (device: AdminDevice): MenuItem[] => [
    { key: 'owner', label: translate('admin.viewOwner'), icon: <User aria-hidden className="h-4 w-4" />, onSelect: () => props.go('users', { id: device.ownerUserId }) },
    { key: 'disconnect', label: translate('admin.disconnect'), icon: <Unplug aria-hidden className="h-4 w-4" />, danger: true, disabled: !device.connected, onSelect: () => void disconnect(device) },
  ];
  const showVersion = data?.items.some((device) => device.version !== undefined) ?? false;
  const showTraffic = data?.items.some((device) => device.traffic7d !== undefined) ?? false;
  const columns: Array<Column<AdminDevice>> = [
    {
      key: 'device',
      header: translate('admin.deviceAndOwner'),
      render: (device) => (
        <RowOpener label={translate('admin.openDevice', { name: device.name })} onOpen={() => setParams({ id: device.id })}>
          <span className="admin-two-line">
            {/* Long host names keep their end (docs/ui-principles.md §14). */}
            <b className="admin-device-name" title={device.name}><MiddleName name={device.name} /></b>
            <span>{device.ownerUsername}</span>
          </span>
        </RowOpener>
      ),
    },
    { key: 'connection', header: translate('admin.connection'), render: (device) => <ConnectionCell device={device} /> },
    { key: 'protection', header: translate('admin.protection'), render: (device) => <ProtectionMark protectedDevice={device.protected} /> },
    ...(showVersion ? [{ key: 'version', header: translate('admin.version'), render: (device: AdminDevice) => <VersionCell device={device} /> }] : []),
    {
      key: 'threads',
      header: translate('admin.threads'),
      className: 'is-numeric',
      render: (device) => (
        <span className="admin-two-line">
          <b>{formatCount(device.threadCount)}</b>
          <span>{translate('admin.agentThreadsCount', { count: device.agentThreadCount })}</span>
        </span>
      ),
    },
    { key: 'ports', header: translate('admin.portMappings'), className: 'is-numeric', render: (device) => (device.portMappings ? formatCount(device.portMappings) : <span className="admin-muted">—</span>) },
    ...(showTraffic ? [{
      key: 'traffic',
      header: translate('admin.traffic7d'),
      render: (device: AdminDevice) => <Sparkline label={translate('admin.traffic7dOf', { name: device.name })} values={device.traffic7d ?? []} />,
    }] : []),
    {
      key: 'menu',
      header: <span className="sr-only">{translate('admin.actions')}</span>,
      className: 'is-menu',
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
        <FilterChips
          label={translate('admin.filterDevices')}
          onChange={(key) => setParams({ filter: key === 'all' ? null : key, id: null })}
          options={FILTERS.filter((key) => key === 'all' || counts?.[key] !== undefined).map((key) => ({
            key,
            label: filterLabel(key),
            count: counts?.[key] ?? null,
            tone: (key === 'unstable' || key === 'unprotected' || key === 'outdated') && (counts?.[key] ?? 0) > 0 ? 'warning' : undefined,
            icon: key === 'online' || key === 'offline' || key === 'unstable' ? <DeviceStateMark showLabel={false} state={key} /> : undefined,
          }))}
          value={filter}
        />
        <SearchField label={translate('admin.searchDevices')} onChange={setText} placeholder={translate('admin.searchDevices')} value={text} />
      </div>
      {error && !data ? <LoadFailure message={error} onRetry={reload} /> : null}
      {!data && !error ? <Skeleton lines={8} /> : null}
      {data ? (
        <Card className="admin-table-card">
          <DataTable
            busy={loading}
            columns={columns}
            empty={translate('admin.noDevicesMatch')}
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
          />
          <Pagination onPage={setPage} page={data.page} pageSize={data.pageSize} total={data.total} />
        </Card>
      ) : null}
      <DeviceDetail id={selectedId} onClose={() => setParams({ id: null })} onDisconnect={disconnect} props={props} />
    </div>
  );
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

function DeviceDetail({ id, onClose, onDisconnect, props }: { id: string | null; onClose: () => void; onDisconnect: (device: AdminDevice) => Promise<boolean>; props: AdminPageProps }) {
  useI18n();
  const { data, error, reload } = useAdminResource<AdminDeviceDetail | null>(
    () => (id ? adminApi.device(id) : Promise.resolve(null)),
    `${id}:${props.refreshKey}`,
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
      onClose={onClose}
      open={Boolean(id)}
      subtitle={device ? translate('admin.deviceSubtitle', { owner: device.ownerUsername, date: formatDate(device.createdAt) }) : null}
      title={device?.name ?? translate('admin.loading')}
    >
      {!detail || !device ? (error ? <LoadFailure message={error} onRetry={reload} /> : <Skeleton lines={6} />) : (
        <>
          {detail.connections !== undefined ? <ConnectionHistory detail={detail} /> : null}
          {detail.traffic !== undefined ? <Traffic detail={detail} /> : null}
          <DetailSection icon={<Info aria-hidden className="h-4 w-4" />} title={translate('admin.details')}>
            <Facts
              items={[
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
                        <li key={share.id}>{scopeLabel(share.scope)} → {share.targetUsername ?? '—'}</li>
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

function ConnectionHistory({ detail }: { detail: AdminDeviceDetail }) {
  useI18n();
  const history = detail.connections;
  if (!history) {
    return (
      <DetailSection icon={<Network aria-hidden className="h-4 w-4" />} title={translate('admin.tunnelHistory')}>
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
    <DetailSection icon={<Network aria-hidden className="h-4 w-4" />} title={translate('admin.tunnelHistory')}>
      <div className="admin-timeline-offline-base">
        <TimelineStrip end={end} formatAt={formatTime} label={translate('admin.tunnelHistoryLabel', { uptime: formatRatio(history.uptime, 1), reconnects: history.reconnects })} spans={spans} start={start} />
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

function Traffic({ detail }: { detail: AdminDeviceDetail }) {
  useI18n();
  const traffic = detail.traffic;
  if (!traffic) {
    return (
      <DetailSection icon={<ArrowLeftRight aria-hidden className="h-4 w-4" />} title={translate('admin.traffic24h')}>
        <p className="admin-empty">{translate('admin.notRecordedYet')}</p>
      </DetailSection>
    );
  }
  return (
    <DetailSection icon={<ArrowLeftRight aria-hidden className="h-4 w-4" />} title={translate('admin.traffic24h')}>
      <ColumnChart
        data={traffic.hours.map((hour) => ({ key: hour.at, label: formatTime(hour.at), values: { requests: hour.requests } }))}
        height={160}
        label={translate('admin.traffic24h')}
        series={[{ key: 'requests', label: translate('admin.requests'), slot: 1 }]}
      />
      <p className="admin-chart-note">{translate('admin.trafficNote')}</p>
      <HistoryNote historyStart={traffic.historyStart} rangeStart={Date.now() - 24 * 3600_000} />
    </DetailSection>
  );
}
