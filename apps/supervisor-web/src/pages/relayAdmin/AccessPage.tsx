// Shares and access: every active share (thread, workspace, whole device) as
// one list with revocation, public links and port previews. No thread titles.
import { confirmAsync, ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ArrowRight, Ban, EyeOff, Folder, Globe, Link2, MessageSquare, Monitor, Share2, Clock } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { adminApi } from './adminApi';
import { DeviceOwnerCell, deviceOwnerText } from './owners';
import { pageWindow, windowLabel } from './ranges';
import { BarList, ChartCard, ColumnChart, ProportionBar, type SeriesSlot } from './charts';
import { formatBytes, formatCount, formatDate, formatDateTime, formatDay, formatRelative, formatTime, parseTime } from './format';
import { accessLabel, filesLabel, scopeLabel, visitLabel } from './labels';
import type { AdminPageProps } from './RelayAdminConsole';
import type { AdminPreviews, AdminPublicLink, AdminShare, AdminShareFilter } from './types';
import { FilterBar, SelectFilter, TimeFilter, UserFilter, windowParams } from './filters';
import { AdminTable, sortRows, useFilterValues, useTableSettings, useUrlSort, type TableColumn } from './table';
import {
  Badge,
  Card,
  errorText,
  Figure,
  FigureBand,
  FilterChips,
  LoadFailure,
  Pagination,
  RowMenu,
  SearchField,
  Skeleton,
  SubTabs,
  useAdminResource,
  useDebounced,
} from './ui';

const PAGE_SIZE = 25;
const FILTER_KEYS = ['scope', 'owner', 'recipient', 'access', 'expiry', 'seen', 'created'] as const;
type FilterKey = (typeof FILTER_KEYS)[number];
type AccessTab = 'shares' | 'links' | 'previews';

export function AccessPage(props: AdminPageProps) {
  useI18n();
  const tab: AccessTab = props.sub === 'links' || props.sub === 'previews' ? props.sub : 'shares';
  return (
    <div className="admin-page">
      <SubTabs
        idPrefix="admin-access"
        label={translate('admin.navShares')}
        onChange={(key) => props.go('shares', { sub: key === 'shares' ? null : key })}
        tabs={[
          { key: 'shares', label: translate('admin.shares'), icon: <Share2 aria-hidden className="h-4 w-4" /> },
          { key: 'links', label: translate('admin.publicLinks'), icon: <Link2 aria-hidden className="h-4 w-4" /> },
          { key: 'previews', label: translate('admin.portPreviews'), icon: <Globe aria-hidden className="h-4 w-4" /> },
        ]}
        value={tab}
      />
      <div aria-labelledby={`admin-access-tab-${tab}`} className="admin-tabpanel" id="admin-access-panel" role="tabpanel">
        {tab === 'shares' ? <Shares {...props} /> : tab === 'links' ? <Links {...props} /> : <Previews {...props} />}
      </div>
    </div>
  );
}

/** Links from before the column filters (`filter=expiring`) still work. */
function legacyFilter(filter: string | null): Partial<Record<FilterKey, string>> {
  switch (filter as AdminShareFilter | null) {
    case 'device':
    case 'workspace':
    case 'thread':
      return { scope: filter as string };
    case 'expiring': return { expiry: 'expiring' };
    case 'unused': return { seen: 'never' };
    default: return {};
  }
}

export function ScopeIcon({ scope }: { scope: string }) {
  if (scope === 'device') return <Monitor aria-hidden className="h-4 w-4" />;
  if (scope === 'workspace') return <Folder aria-hidden className="h-4 w-4" />;
  return <MessageSquare aria-hidden className="h-4 w-4" />;
}

// Five series at most: opening a device and opening a thread are one "opened".
const KIND_SLOTS: Record<string, SeriesSlot> = { open: 1, send_prompt: 2, read_workspace_file: 3, write_workspace_file: 4, create_thread: 5 };
const CHART_KINDS = ['open', 'send_prompt', 'read_workspace_file', 'write_workspace_file', 'create_thread'] as const;

function chartKinds(kinds: Record<string, number>) {
  const merged: Record<string, number> = { ...kinds, open: (kinds.open_thread ?? 0) + (kinds.open_device ?? 0) };
  delete merged.open_thread;
  delete merged.open_device;
  return merged;
}

function kindLabel(kind: string) {
  return kind === 'open' ? translate('admin.visitOpen') : visitLabel(kind);
}

function Shares(props: AdminPageProps) {
  const { refreshKey, onLoaded, onUnauthorized, setParams } = props;
  const values = useFilterValues(props, FILTER_KEYS, legacyFilter);
  const urlSort = useUrlSort(props);
  const settings = useTableSettings('shares', { urlSort });
  const [text, setText] = useState('');
  const q = useDebounced(text.trim());
  const [page, setPage] = useState(1);
  const query = useMemo(() => ({
    q,
    scope: values.scope,
    owner: values.owner,
    recipient: values.recipient,
    access: values.access,
    expiry: values.expiry,
    ...windowParams(values.seen, 'seen'),
    ...windowParams(values.created, 'created'),
    sort: settings.sort?.key,
    dir: settings.sort?.dir,
  }), [q, values, settings.sort?.key, settings.sort?.dir]);
  const queryKey = JSON.stringify(query);
  useEffect(() => setPage(1), [queryKey]);
  // The usage chart follows the page's range; the list does not.
  const chartWindow = useMemo(() => pageWindow(props), [props.range, props.customRange]);
  const { data, error, loading, reload } = useAdminResource(
    () => adminApi.shares({ ...query, ...chartWindow, page, pageSize: PAGE_SIZE }),
    `${queryKey}:${JSON.stringify(chartWindow)}:${page}:${refreshKey}`,
    onUnauthorized,
  );
  useEffect(() => {
    if (data) onLoaded();
  }, [data, onLoaded]);
  const setFilter = (key: FilterKey, value: string | null) => setParams({ [key]: value, filter: null });
  const active = FILTER_KEYS.filter((key) => values[key]).length;
  const [failure, setFailure] = useState<{ id: string; message: string } | null>(null);
  const revoke = async (share: AdminShare) => {
    const confirmed = await confirmAsync({
      title: translate('admin.revokeTitle', { owner: share.ownerUsername, target: share.targetUsername }),
      description: translate('admin.revokeBody', { scope: scopeLabel(share.scope), device: share.deviceName }),
      confirmLabel: translate('admin.revoke'),
    });
    if (!confirmed) return;
    setFailure(null);
    try {
      await adminApi.revokeShare(share.kind, share.id);
      reload();
      props.onChanged();
    } catch (caught) {
      setFailure({ id: share.id, message: errorText(caught) });
    }
  };
  const now = Date.now();
  const person = (id: string, name: string, email?: string | null) => <button className="admin-text-link" onClick={() => props.go('users', { id })} title={email ?? undefined} type="button">{name}</button>;
  const columns: Array<TableColumn<AdminShare>> = [
    { key: 'scope', label: translate('admin.scope'), sortable: true, width: 150, render: (share) => <span className="admin-scope"><ScopeIcon scope={share.scope} />{scopeLabel(share.scope)}</span> },
    {
      key: 'people',
      label: translate('admin.ownerToRecipient'),
      hideable: false,
      width: 220,
      render: (share) => (
        <span className="admin-route">
          {person(share.ownerUserId, share.ownerUsername, share.ownerEmail)}
          <ArrowRight aria-label={translate('admin.sharedWith')} className="h-3.5 w-3.5" />
          {person(share.targetUserId, share.targetUsername, share.targetEmail)}
        </span>
      ),
    },
    { key: 'owner', label: translate('admin.owner'), sortable: true, defaultHidden: true, width: 140, render: (share) => person(share.ownerUserId, share.ownerUsername) },
    { key: 'recipient', label: translate('admin.recipient'), sortable: true, defaultHidden: true, width: 140, render: (share) => person(share.targetUserId, share.targetUsername) },
    {
      key: 'device',
      label: translate('admin.deviceAndPlace'),
      sortable: true,
      width: 220,
      // The shared device is the owner's: named with them, and the workspace.
      render: (share) => (
        <DeviceOwnerCell
          detail={share.workspaceLabel ? <span className="admin-truncate" title={share.workspaceLabel}>{share.workspaceLabel}</span> : null}
          device={{ deviceId: share.deviceId, name: share.deviceName, ownerUserId: share.ownerUserId, ownerUsername: share.ownerUsername, ownerEmail: share.ownerEmail }}
          onOpenDevice={(id) => props.go('devices', { id })}
          onOpenUser={(id) => props.go('users', { id })}
        />
      ),
    },
    {
      key: 'access',
      label: translate('admin.permissions'),
      sortable: true,
      firstDir: 'desc',
      width: 220,
      render: (share) => (
        <span className="admin-pills">
          <span className="admin-pill">{accessLabel(share.threadAccess)}</span>
          {share.workspaceAccess !== 'none' ? <span className="admin-pill">{filesLabel(share.workspaceAccess)}</span> : null}
          {share.canCreateThreads ? <span className="admin-pill">{translate('admin.canCreateThreads')}</span> : null}
        </span>
      ),
    },
    { key: 'lastUsed', label: translate('admin.lastUsed'), sortable: true, firstDir: 'desc', width: 180, render: (share) => (share.lastAccessedAt ? <span title={formatDateTime(share.lastAccessedAt)}>{`${formatRelative(share.lastAccessedAt)} · ${visitLabel(share.lastAccessKind)}`}</span> : <span className="admin-muted">{translate('admin.neverUsed')}</span>) },
    { key: 'expires', label: translate('admin.expires'), sortable: true, width: 170, render: (share) => <Expiry now={now} share={share} /> },
    { key: 'created', label: translate('admin.created'), sortable: true, firstDir: 'desc', defaultHidden: true, width: 130, render: (share) => <time dateTime={share.createdAt} title={formatDateTime(share.createdAt)}>{formatDate(share.createdAt)}</time> },
    {
      key: 'menu',
      label: translate('admin.actions'),
      header: <span className="sr-only">{translate('admin.actions')}</span>,
      className: 'is-menu',
      hideable: false,
      fixed: true,
      width: 64,
      render: (share) => (
        <span className="admin-inline">
          <RowMenu
            items={[{ key: 'revoke', label: translate('admin.revoke'), icon: <Ban aria-hidden className="h-4 w-4" />, danger: true, onSelect: () => void revoke(share) }]}
            label={translate('admin.shareActions', { owner: share.ownerUsername, target: share.targetUsername })}
          />
          {failure?.id === share.id ? <ErrorMark message={failure.message} /> : null}
        </span>
      ),
    },
  ];
  const summary = data?.summary;
  const usage = data?.usage;
  const days = usage?.days.map((day) => ({ day: day.day, at: day.at, kinds: chartKinds(day.kinds) })) ?? [];
  const kinds = CHART_KINDS.filter((kind) => days.some((day) => day.kinds[kind]));
  return (
    <>
      {summary && usage ? (
        <div className="admin-grid is-wide-right">
          <Card className="admin-share-summary">
            <Figure
              detail={summary.createdInRange !== undefined && data?.window
                ? translate('admin.sharesFigureDetailIn', { added: formatCount(summary.createdInRange), revoked: formatCount(summary.revokedInRange ?? 0), expiring: formatCount(summary.expiringSoon), range: windowLabel(data.window) })
                : translate('admin.sharesFigureDetail', { added: formatCount(summary.createdThisWeek), revoked: formatCount(summary.revokedThisWeek), expiring: formatCount(summary.expiringSoon) })}
              label={translate('admin.activeShares')}
              value={formatCount(summary.active)}
            />
            <ProportionBar
              items={[
                { key: 'device', label: translate('admin.scopeDevice'), value: summary.byScope.device, slot: 1 },
                { key: 'workspace', label: translate('admin.scopeWorkspace'), value: summary.byScope.workspace, slot: 2 },
                { key: 'thread', label: translate('admin.scopeThread'), value: summary.byScope.thread, slot: 3 },
              ]}
              label={translate('admin.byScope')}
            />
            <h4 className="admin-subheading">{translate('admin.permissions')}</h4>
            <BarList
              items={[
                { key: 'control', label: translate('admin.accessControl'), value: summary.permissions.control },
                { key: 'read', label: translate('admin.accessRead'), value: summary.permissions.read },
                { key: 'files', label: translate('admin.filesWrite'), value: summary.permissions.filesWrite },
                { key: 'create', label: translate('admin.canCreateThreads'), value: summary.permissions.canCreateThreads },
              ]}
              label={translate('admin.permissions')}
              max={summary.active}
            />
          </Card>
          <ChartCard
            aside={usage.approximate ? <Badge tone="info">{translate('admin.approximate')}</Badge> : null}
            subtitle={data?.window ? translate('admin.usageSubtitleIn', { range: windowLabel(data.window), unit: (usage.resolutionSeconds ?? 86_400) < 86_400 ? translate('admin.perHour') : translate('admin.perDay') }) : translate('admin.usageSubtitle')}
            table={{ columns: [translate('admin.time'), ...kinds.map(kindLabel)], rows: days.map((day) => [day.at ? formatDateTime(day.at) : formatDay(day.day), ...kinds.map((kind) => day.kinds[kind] ?? 0)]) }}
            title={translate('admin.recipientUsage')}
          >
            <ColumnChart
              data={days.map((day) => ({ key: day.at ?? day.day, label: day.at ? formatTime(day.at) : formatDay(day.day), values: day.kinds }))}
              height={200}
              label={translate('admin.recipientUsage')}
              series={kinds.map((kind) => ({ key: kind, label: kindLabel(kind), slot: KIND_SLOTS[kind] ?? 1 }))}
            />
            {kinds.length ? <LegendRow kinds={kinds} /> : <p className="admin-empty">{translate('admin.noVisits')}</p>}
          </ChartCard>
        </div>
      ) : null}
      <div className="admin-toolbar">
        <FilterBar active={active} label={translate('admin.filterShares')} onClear={() => setParams({ ...Object.fromEntries(FILTER_KEYS.map((key) => [key, null])), filter: null })}>
          <SelectFilter
            label={translate('admin.scope')}
            onChange={(value) => setFilter('scope', value)}
            options={(['device', 'workspace', 'thread'] as const).map((scope) => ({ value: scope, label: scopeLabel(scope), count: data?.counts[scope] ?? null, icon: <ScopeIcon scope={scope} /> }))}
            value={values.scope}
          />
          <UserFilter label={translate('admin.owner')} onChange={(value) => setFilter('owner', value)} value={values.owner} valueLabel={data?.labels?.owner} />
          <UserFilter label={translate('admin.recipient')} onChange={(value) => setFilter('recipient', value)} value={values.recipient} valueLabel={data?.labels?.recipient} />
          <SelectFilter
            label={translate('admin.permissions')}
            onChange={(value) => setFilter('access', value)}
            options={[
              { value: 'control', label: translate('admin.accessControl'), count: data?.summary.permissions.control },
              { value: 'read', label: translate('admin.accessRead'), count: data?.summary.permissions.read },
              { value: 'filesWrite', label: translate('admin.filesWrite'), count: data?.summary.permissions.filesWrite },
              { value: 'filesRead', label: translate('admin.filesRead') },
              { value: 'createThreads', label: translate('admin.canCreateThreads'), count: data?.summary.permissions.canCreateThreads },
            ]}
            value={values.access}
          />
          <SelectFilter
            label={translate('admin.expires')}
            onChange={(value) => setFilter('expiry', value)}
            options={[
              { value: 'expiring', label: translate('admin.expiringSoon'), count: data?.counts.expiring, icon: <Clock aria-hidden className="h-3.5 w-3.5" /> },
              { value: 'set', label: translate('admin.hasExpiry') },
              { value: 'none', label: translate('admin.noExpiry') },
            ]}
            value={values.expiry}
          />
          <TimeFilter label={translate('admin.lastUsed')} never older onChange={(value) => setFilter('seen', value)} value={values.seen} />
          <TimeFilter label={translate('admin.created')} onChange={(value) => setFilter('created', value)} value={values.created} />
        </FilterBar>
        <SearchField label={translate('admin.searchShares')} onChange={setText} placeholder={translate('admin.searchShares')} value={text} />
      </div>
      {error && !data ? <LoadFailure message={error} onRetry={reload} /> : null}
      {!data && !error ? <Skeleton lines={8} /> : null}
      {data ? (
        <Card className="admin-table-card">
          <AdminTable
            busy={loading}
            columns={columns}
            empty={translate('admin.noSharesMatch')}
            grow="people"
            id="shares"
            label={translate('admin.shares')}
            phoneRow={(share) => (
              <>
                <span className="admin-scope-icon"><ScopeIcon scope={share.scope} /></span>
                <span className="admin-two-line">
                  <b>{share.ownerUsername} → {share.targetUsername}</b>
                  <span>{scopeLabel(share.scope)} · {deviceOwnerText({ name: share.deviceName, ownerUsername: share.ownerUsername })} · {accessLabel(share.threadAccess)}</span>
                  <span>{share.lastAccessedAt ? formatRelative(share.lastAccessedAt) : translate('admin.neverUsed')}</span>
                </span>
                <RowMenu
                  items={[{ key: 'revoke', label: translate('admin.revoke'), icon: <Ban aria-hidden className="h-4 w-4" />, danger: true, onSelect: () => void revoke(share) }]}
                  label={translate('admin.shareActions', { owner: share.ownerUsername, target: share.targetUsername })}
                />
              </>
            )}
            rowKey={(share) => `${share.kind}:${share.id}`}
            rows={data.items}
            settings={settings}
            total={data.total}
          />
          <Pagination onPage={setPage} page={data.page} pageSize={data.pageSize} total={data.total} />
        </Card>
      ) : null}
      <p className="admin-footnote">
        <EyeOff aria-hidden className="h-4 w-4" />
        {translate('admin.sharesFootnote')}
      </p>
    </>
  );
}

function LegendRow({ kinds }: { kinds: readonly string[] }) {
  return (
    <ul className="admin-legend">
      {kinds.map((kind) => (
        <li key={kind}>
          <span aria-hidden className={`admin-swatch admin-series-${KIND_SLOTS[kind] ?? 1}`} />
          <span>{kindLabel(kind)}</span>
        </li>
      ))}
    </ul>
  );
}

function Expiry({ share, now }: { share: AdminShare; now: number }) {
  useI18n();
  const expires = parseTime(share.expiresAt);
  if (expires === null) return <span className="admin-muted">{translate('admin.noExpiry')}</span>;
  const soon = expires - now < 7 * 86_400_000;
  return (
    <span className="admin-inline">
      <span title={formatDateTime(share.expiresAt)}>{formatRelative(share.expiresAt)}</span>
      {soon ? <Badge tone="warning">{translate('admin.expiringSoon')}</Badge> : null}
    </span>
  );
}

function Links(props: AdminPageProps) {
  useI18n();
  const filter = ['legacy', 'encrypted', 'live'].includes(props.filter ?? '') ? (props.filter as string) : 'all';
  const urlSort = useUrlSort(props);
  const settings = useTableSettings('public-links', { urlSort });
  const [page, setPage] = useState(1);
  const sortKey = `${settings.sort?.key ?? ''}:${settings.sort?.dir ?? ''}`;
  useEffect(() => setPage(1), [filter, sortKey]);
  const { data, error, loading, reload } = useAdminResource(
    () => adminApi.publicLinks({ filter, page, pageSize: PAGE_SIZE, sort: settings.sort?.key, dir: settings.sort?.dir }),
    `${filter}:${page}:${sortKey}:${props.refreshKey}`,
    props.onUnauthorized,
  );
  const loaded = props.onLoaded;
  useEffect(() => {
    if (data) loaded();
  }, [data, loaded]);
  if (!data) return error ? <LoadFailure message={error} onRetry={reload} /> : <Skeleton lines={6} />;
  const summary = data.summary;
  const columns: Array<TableColumn<AdminPublicLink>> = [
    {
      key: 'owner',
      label: translate('admin.deviceAndOwner'),
      sortable: true,
      hideable: false,
      width: 240,
      render: (link) => (
        <DeviceOwnerCell
          device={{ deviceId: link.deviceId ?? '', name: link.deviceName, ownerUserId: link.ownerUserId, ownerUsername: link.ownerUsername, ownerEmail: link.ownerEmail }}
          onOpenDevice={link.deviceId ? (id) => props.go('devices', { id }) : undefined}
          onOpenUser={(id) => props.go('users', { id })}
        />
      ),
    },
    { key: 'storage', label: translate('admin.storage'), sortable: true, width: 170, render: (link) => (link.encrypted ? <Badge tone="success">{translate('admin.encrypted')}</Badge> : <Badge tone="warning">{translate('admin.legacyPlaintext')}</Badge>) },
    { key: 'live', label: translate('admin.updates'), sortable: true, width: 130, render: (link) => (link.live ? translate('admin.liveLink') : translate('admin.staticLink')) },
    { key: 'created', label: translate('admin.created'), sortable: true, firstDir: 'desc', width: 140, render: (link) => formatDate(link.createdAt) },
    { key: 'refreshed', label: translate('admin.refreshed'), sortable: true, firstDir: 'desc', width: 140, render: (link) => (link.refreshedAt ? formatRelative(link.refreshedAt) : '—') },
    { key: 'size', label: translate('admin.size'), sortable: true, firstDir: 'desc', numeric: true, width: 110, render: (link) => formatBytes(link.bytes) },
  ];
  return (
    <>
      <FigureBand label={translate('admin.publicLinks')}>
        <Figure label={translate('admin.publicLinks')} value={formatCount(summary.total)} detail={translate('admin.linksTotalDetail', { size: formatBytes(summary.bytes) })} />
        <Figure label={translate('admin.encrypted')} value={formatCount(summary.encrypted)} />
        <Figure label={translate('admin.legacyPlaintext')} value={formatCount(summary.legacy)} detail={summary.legacy ? translate('admin.legacyDetail') : undefined} />
        <Figure label={translate('admin.liveLink')} value={formatCount(summary.live)} />
      </FigureBand>
      <div className="admin-toolbar">
        <FilterChips
          label={translate('admin.filterLinks')}
          onChange={(key) => props.setParams({ filter: key === 'all' ? null : key })}
          options={[
            { key: 'all', label: translate('admin.filterAll'), count: summary.total },
            { key: 'legacy', label: translate('admin.legacyPlaintext'), count: summary.legacy, tone: summary.legacy ? 'warning' : undefined },
            { key: 'encrypted', label: translate('admin.encrypted'), count: summary.encrypted },
            { key: 'live', label: translate('admin.liveLink'), count: summary.live },
          ]}
          value={filter}
        />
      </div>
      <Card className="admin-table-card">
        <AdminTable
          busy={loading}
          columns={columns}
          empty={translate('admin.noLinks')}
          id="public-links"
          label={translate('admin.publicLinks')}
          settings={settings}
          phoneRow={(link) => (
            <>
              <span className="admin-two-line">
                <b>{deviceOwnerText({ name: link.deviceName, ownerUsername: link.ownerUsername })}</b>
                <span>{formatDate(link.createdAt)} · {formatBytes(link.bytes)}</span>
              </span>
              {link.encrypted ? <Badge tone="success">{translate('admin.encrypted')}</Badge> : <Badge tone="warning">{translate('admin.legacyPlaintext')}</Badge>}
            </>
          )}
          rowKey={(link) => link.id}
          rows={data.items}
        />
        <Pagination onPage={setPage} page={data.page} pageSize={data.pageSize} total={data.total} />
      </Card>
      <p className="admin-footnote">
        <EyeOff aria-hidden className="h-4 w-4" />
        {translate('admin.linksFootnote')}
      </p>
    </>
  );
}

type PreviewRow = AdminPreviews['devices'][number];

function Previews(props: AdminPageProps) {
  useI18n();
  const settings = useTableSettings('previews', { defaultSort: { key: 'mappings', dir: 'desc' } });
  const { data, error, reload } = useAdminResource(() => adminApi.previews(), `previews:${props.refreshKey}`, props.onUnauthorized);
  const loaded = props.onLoaded;
  useEffect(() => {
    if (data) loaded();
  }, [data, loaded]);
  if (!data) return error ? <LoadFailure message={error} onRetry={reload} /> : <Skeleton lines={4} />;
  const text = (value: string | null | undefined) => (value ?? '').toLowerCase();
  const previewColumns: Array<TableColumn<PreviewRow>> = [
    {
      key: 'device',
      label: translate('admin.deviceAndOwner'),
      hideable: false,
      width: 240,
      compare: (a, b) => text(a.deviceName).localeCompare(text(b.deviceName)),
      render: (row) => (
        <DeviceOwnerCell
          device={{ deviceId: row.deviceId, name: row.deviceName, ownerUserId: row.ownerUserId, ownerUsername: row.ownerUsername, ownerEmail: row.ownerEmail }}
          onOpenDevice={(id) => props.go('devices', { id })}
          onOpenUser={(id) => props.go('users', { id })}
        />
      ),
    },
    { key: 'mappings', label: translate('admin.previewMappings'), numeric: true, firstDir: 'desc', width: 140, compare: (a, b) => a.mappings - b.mappings, render: (row) => formatCount(row.mappings) },
    { key: 'sessions', label: translate('admin.previewSessions'), numeric: true, firstDir: 'desc', width: 140, compare: (a, b) => a.sessions - b.sessions, render: (row) => formatCount(row.sessions) },
    { key: 'streams', label: translate('admin.previewStreams'), numeric: true, firstDir: 'desc', width: 140, compare: (a, b) => a.streams - b.streams, render: (row) => formatCount(row.streams) },
  ];
  return (
    <>
      <FigureBand label={translate('admin.portPreviews')}>
        <Figure label={translate('admin.previewMappings')} value={formatCount(data.mappings)} detail={data.configured ? undefined : translate('admin.previewsNotConfigured')} />
        <Figure label={translate('admin.previewSessions')} value={formatCount(data.sessions)} />
        <Figure label={translate('admin.previewStreams')} value={formatCount(data.streams)} />
      </FigureBand>
      <Card className="admin-table-card" subtitle={translate('admin.previewsSubtitle')} title={translate('admin.previewsByDevice')}>
        <AdminTable
          columns={previewColumns}
          empty={translate('admin.noPreviews')}
          id="previews"
          label={translate('admin.previewsByDevice')}
          onOpen={(row) => props.go('devices', { id: row.deviceId })}
          phoneRow={(row) => (
            <span className="admin-two-line">
              <b>{deviceOwnerText({ name: row.deviceName, ownerUsername: row.ownerUsername })}</b>
              <span>{translate('admin.previewRow', { mappings: row.mappings, sessions: row.sessions })}</span>
            </span>
          )}
          settings={settings}
          rowKey={(row) => row.deviceId}
          rows={sortRows(data.devices, previewColumns, settings.sort)}
        />
      </Card>
    </>
  );
}
