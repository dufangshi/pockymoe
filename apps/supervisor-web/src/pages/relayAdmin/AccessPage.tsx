// Shares and access: every active share (thread, workspace, whole device) as
// one list with revocation, public links and port previews. No thread titles.
import { confirmAsync, ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ArrowRight, Ban, EyeOff, Folder, Globe, Link2, MessageSquare, Monitor, Share2, Clock } from 'lucide-react';
import { useEffect, useState } from 'react';
import { adminApi } from './adminApi';
import { BarList, ChartCard, ColumnChart, ProportionBar, type SeriesSlot } from './charts';
import { formatBytes, formatCount, formatDate, formatDateTime, formatDay, formatRelative, parseTime } from './format';
import { accessLabel, filesLabel, scopeLabel, visitLabel } from './labels';
import type { AdminPageProps } from './RelayAdminConsole';
import type { AdminPublicLink, AdminShare, AdminShareFilter } from './types';
import {
  Badge,
  Card,
  DataTable,
  errorText,
  Figure,
  FigureBand,
  FilterChips,
  LoadFailure,
  Pagination,
  RowMenu,
  RowOpener,
  SearchField,
  Skeleton,
  SubTabs,
  useAdminResource,
  useDebounced,
  type Column,
} from './ui';

const PAGE_SIZE = 25;
const SHARE_FILTERS: AdminShareFilter[] = ['all', 'device', 'workspace', 'thread', 'expiring', 'unused'];
type AccessTab = 'shares' | 'links' | 'previews';

export function AccessPage(props: AdminPageProps) {
  useI18n();
  const tab: AccessTab = props.sub === 'links' || props.sub === 'previews' ? props.sub : 'shares';
  return (
    <div className="admin-page">
      <SubTabs
        idPrefix="admin-access"
        label={translate('admin.navShares')}
        onChange={(key) => props.setParams({ sub: key === 'shares' ? null : key, filter: null, id: null })}
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

function shareFilterLabel(filter: AdminShareFilter) {
  switch (filter) {
    case 'all': return translate('admin.filterAll');
    case 'device': return translate('admin.scopeDevice');
    case 'workspace': return translate('admin.scopeWorkspace');
    case 'thread': return translate('admin.scopeThread');
    case 'expiring': return translate('admin.expiringSoon');
    case 'unused': return translate('admin.neverUsed');
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
  const filter = (SHARE_FILTERS.includes(props.filter as AdminShareFilter) ? props.filter : 'all') as AdminShareFilter;
  const [text, setText] = useState('');
  const q = useDebounced(text.trim());
  const [page, setPage] = useState(1);
  useEffect(() => setPage(1), [filter, q]);
  const { data, error, loading, reload } = useAdminResource(
    () => adminApi.shares({ filter, q, page, pageSize: PAGE_SIZE }),
    `${filter}:${q}:${page}:${refreshKey}`,
    onUnauthorized,
  );
  useEffect(() => {
    if (data) onLoaded();
  }, [data, onLoaded]);
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
  const columns: Array<Column<AdminShare>> = [
    { key: 'scope', header: translate('admin.scope'), render: (share) => <span className="admin-scope"><ScopeIcon scope={share.scope} />{scopeLabel(share.scope)}</span> },
    {
      key: 'people',
      header: translate('admin.ownerToRecipient'),
      render: (share) => (
        <span className="admin-route">
          <button className="admin-text-link" onClick={() => props.go('users', { id: share.ownerUserId })} type="button">{share.ownerUsername}</button>
          <ArrowRight aria-label={translate('admin.sharedWith')} className="h-3.5 w-3.5" />
          <button className="admin-text-link" onClick={() => props.go('users', { id: share.targetUserId })} type="button">{share.targetUsername}</button>
        </span>
      ),
    },
    { key: 'device', header: translate('admin.deviceAndPlace'), render: (share) => <span className="admin-truncate">{share.workspaceLabel ? `${share.deviceName} · ${share.workspaceLabel}` : share.deviceName}</span> },
    {
      key: 'access',
      header: translate('admin.permissions'),
      render: (share) => (
        <span className="admin-pills">
          <span className="admin-pill">{accessLabel(share.threadAccess)}</span>
          {share.workspaceAccess !== 'none' ? <span className="admin-pill">{filesLabel(share.workspaceAccess)}</span> : null}
          {share.canCreateThreads ? <span className="admin-pill">{translate('admin.canCreateThreads')}</span> : null}
        </span>
      ),
    },
    { key: 'used', header: translate('admin.lastUsed'), render: (share) => (share.lastAccessedAt ? `${formatRelative(share.lastAccessedAt)} · ${visitLabel(share.lastAccessKind)}` : <span className="admin-muted">{translate('admin.neverUsed')}</span>) },
    { key: 'expiry', header: translate('admin.expires'), render: (share) => <Expiry now={now} share={share} /> },
    {
      key: 'menu',
      header: <span className="sr-only">{translate('admin.actions')}</span>,
      className: 'is-menu',
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
  const days = usage?.days.map((day) => ({ day: day.day, kinds: chartKinds(day.kinds) })) ?? [];
  const kinds = CHART_KINDS.filter((kind) => days.some((day) => day.kinds[kind]));
  return (
    <>
      {summary && usage ? (
        <div className="admin-grid is-wide-right">
          <Card className="admin-share-summary">
            <Figure
              detail={translate('admin.sharesFigureDetail', { added: formatCount(summary.createdThisWeek), revoked: formatCount(summary.revokedThisWeek), expiring: formatCount(summary.expiringSoon) })}
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
            subtitle={translate('admin.usageSubtitle')}
            table={{ columns: [translate('admin.day'), ...kinds.map(kindLabel)], rows: days.map((day) => [formatDay(day.day), ...kinds.map((kind) => day.kinds[kind] ?? 0)]) }}
            title={translate('admin.recipientUsage')}
          >
            <ColumnChart
              data={days.map((day) => ({ key: day.day, label: formatDay(day.day), values: day.kinds }))}
              height={200}
              label={translate('admin.recipientUsage')}
              series={kinds.map((kind) => ({ key: kind, label: kindLabel(kind), slot: KIND_SLOTS[kind] ?? 1 }))}
            />
            {kinds.length ? <LegendRow kinds={kinds} /> : <p className="admin-empty">{translate('admin.noVisits')}</p>}
          </ChartCard>
        </div>
      ) : null}
      <div className="admin-toolbar">
        <FilterChips
          label={translate('admin.filterShares')}
          onChange={(key) => setParams({ filter: key === 'all' ? null : key })}
          options={SHARE_FILTERS.map((key) => ({
            key,
            label: shareFilterLabel(key),
            count: data?.counts[key] ?? null,
            tone: key === 'expiring' && (data?.counts.expiring ?? 0) > 0 ? 'warning' : undefined,
            icon: key === 'expiring' ? <Clock aria-hidden className="h-3.5 w-3.5" /> : undefined,
          }))}
          value={filter}
        />
        <SearchField label={translate('admin.searchShares')} onChange={setText} placeholder={translate('admin.searchShares')} value={text} />
      </div>
      {error && !data ? <LoadFailure message={error} onRetry={reload} /> : null}
      {!data && !error ? <Skeleton lines={8} /> : null}
      {data ? (
        <Card className="admin-table-card">
          <DataTable
            busy={loading}
            columns={columns}
            empty={translate('admin.noSharesMatch')}
            label={translate('admin.shares')}
            phoneRow={(share) => (
              <>
                <span className="admin-scope-icon"><ScopeIcon scope={share.scope} /></span>
                <span className="admin-two-line">
                  <b>{share.ownerUsername} → {share.targetUsername}</b>
                  <span>{scopeLabel(share.scope)} · {share.deviceName} · {accessLabel(share.threadAccess)}</span>
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
  const [page, setPage] = useState(1);
  useEffect(() => setPage(1), [filter]);
  const { data, error, loading, reload } = useAdminResource(
    () => adminApi.publicLinks({ filter, page, pageSize: PAGE_SIZE }),
    `${filter}:${page}:${props.refreshKey}`,
    props.onUnauthorized,
  );
  const loaded = props.onLoaded;
  useEffect(() => {
    if (data) loaded();
  }, [data, loaded]);
  if (!data) return error ? <LoadFailure message={error} onRetry={reload} /> : <Skeleton lines={6} />;
  const summary = data.summary;
  const columns: Array<Column<AdminPublicLink>> = [
    { key: 'owner', header: translate('admin.owner'), render: (link) => <span className="admin-two-line"><b>{link.ownerUsername}</b><span>{link.deviceName}</span></span> },
    { key: 'kind', header: translate('admin.storage'), render: (link) => (link.encrypted ? <Badge tone="success">{translate('admin.encrypted')}</Badge> : <Badge tone="warning">{translate('admin.legacyPlaintext')}</Badge>) },
    { key: 'live', header: translate('admin.updates'), render: (link) => (link.live ? translate('admin.liveLink') : translate('admin.staticLink')) },
    { key: 'created', header: translate('admin.created'), render: (link) => formatDate(link.createdAt) },
    { key: 'refreshed', header: translate('admin.refreshed'), render: (link) => (link.refreshedAt ? formatRelative(link.refreshedAt) : '—') },
    { key: 'size', header: translate('admin.size'), className: 'is-numeric', render: (link) => formatBytes(link.bytes) },
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
        <DataTable
          busy={loading}
          columns={columns}
          empty={translate('admin.noLinks')}
          label={translate('admin.publicLinks')}
          phoneRow={(link) => (
            <>
              <span className="admin-two-line">
                <b>{link.ownerUsername} · {link.deviceName}</b>
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

function Previews(props: AdminPageProps) {
  useI18n();
  const { data, error, reload } = useAdminResource(() => adminApi.previews(), `previews:${props.refreshKey}`, props.onUnauthorized);
  const loaded = props.onLoaded;
  useEffect(() => {
    if (data) loaded();
  }, [data, loaded]);
  if (!data) return error ? <LoadFailure message={error} onRetry={reload} /> : <Skeleton lines={4} />;
  return (
    <>
      <FigureBand label={translate('admin.portPreviews')}>
        <Figure label={translate('admin.previewMappings')} value={formatCount(data.mappings)} detail={data.configured ? undefined : translate('admin.previewsNotConfigured')} />
        <Figure label={translate('admin.previewSessions')} value={formatCount(data.sessions)} />
        <Figure label={translate('admin.previewStreams')} value={formatCount(data.streams)} />
      </FigureBand>
      <Card className="admin-table-card" subtitle={translate('admin.previewsSubtitle')} title={translate('admin.previewsByDevice')}>
        <DataTable
          columns={[
            {
              key: 'device',
              header: translate('admin.deviceAndOwner'),
              render: (row) => (
                <RowOpener label={translate('admin.openDevice', { name: row.deviceName })} onOpen={() => props.go('devices', { id: row.deviceId })}>
                  <span className="admin-two-line"><b>{row.deviceName}</b><span>{row.ownerUsername ?? '—'}</span></span>
                </RowOpener>
              ),
            },
            { key: 'mappings', header: translate('admin.previewMappings'), className: 'is-numeric', render: (row) => formatCount(row.mappings) },
            { key: 'sessions', header: translate('admin.previewSessions'), className: 'is-numeric', render: (row) => formatCount(row.sessions) },
            { key: 'streams', header: translate('admin.previewStreams'), className: 'is-numeric', render: (row) => formatCount(row.streams) },
          ]}
          empty={translate('admin.noPreviews')}
          label={translate('admin.previewsByDevice')}
          onOpen={(row) => props.go('devices', { id: row.deviceId })}
          phoneRow={(row) => (
            <span className="admin-two-line">
              <b>{row.deviceName}</b>
              <span>{translate('admin.previewRow', { mappings: row.mappings, sessions: row.sessions })}</span>
            </span>
          )}
          rowKey={(row) => row.deviceId}
          rows={data.devices}
        />
      </Card>
    </>
  );
}
