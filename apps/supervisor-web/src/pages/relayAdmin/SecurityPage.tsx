// Sessions and security: the relay-wide security audit, live sessions, the
// admin action log and rate limiting. The audit holds account IDs, event
// names, resource IDs and times only.
import { confirmAsync, ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { Gauge, History, Laptop, ScrollText, ShieldCheck } from 'lucide-react';
import { useEffect, useState } from 'react';
import { adminApi } from './adminApi';
import { BarList, ChartCard, ColumnChart, HistoryNote, Legend, LineChart, type SeriesSlot } from './charts';
import { EventIcon } from './events';
import { formatCount, formatDateTime, formatDay, formatPercent, formatRelative } from './format';
import { actionLabel, eventCategoryLabel, eventLabel, rangeLabel } from './labels';
import type { AdminPageProps } from './RelayAdminConsole';
import type { AdminAction, AdminSecurityEvent, AdminSecuritySummary, AdminSession } from './types';
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
  SearchField,
  Skeleton,
  SubTabs,
  useAdminResource,
  useDebounced,
  type Column,
} from './ui';

type SecurityTab = 'events' | 'sessions' | 'actions' | 'limits';
const CATEGORY_SLOTS: Record<string, SeriesSlot> = { login: 1, challenge: 2, reauth: 3, credential: 4, failure: 5, activity: 1 };
const CHART_CATEGORIES = ['login', 'challenge', 'reauth', 'credential'] as const;
const EVENT_FILTERS = ['all', 'login', 'challenge', 'reauth', 'credential', 'failure', 'activity'] as const;

export function SecurityPage(props: AdminPageProps) {
  useI18n();
  const summary = useAdminResource(() => adminApi.security(props.range), `${props.range}:${props.refreshKey}`, props.onUnauthorized);
  const recordsActions = summary.data?.adminActions !== undefined;
  const recordsLimits = summary.data?.rateLimited !== undefined;
  const requested = (props.sub ?? 'events') as SecurityTab;
  const tab: SecurityTab = requested === 'sessions' || (requested === 'actions' && recordsActions) || (requested === 'limits' && recordsLimits) ? requested : 'events';
  const loaded = props.onLoaded;
  useEffect(() => {
    if (summary.data) loaded();
  }, [summary.data, loaded]);
  return (
    <div className="admin-page">
      <SubTabs
        idPrefix="admin-security"
        label={translate('admin.navSecurity')}
        onChange={(key) => props.setParams({ sub: key === 'events' ? null : key, filter: null })}
        tabs={[
          { key: 'events', label: translate('admin.securityEvents'), icon: <ScrollText aria-hidden className="h-4 w-4" /> },
          { key: 'sessions', label: translate('admin.sessions'), count: summary.data?.activeSessions ?? null, icon: <Laptop aria-hidden className="h-4 w-4" /> },
          ...(recordsActions ? [{ key: 'actions' as const, label: translate('admin.adminActions'), icon: <History aria-hidden className="h-4 w-4" /> }] : []),
          ...(recordsLimits ? [{ key: 'limits' as const, label: translate('admin.rateLimits'), icon: <Gauge aria-hidden className="h-4 w-4" /> }] : []),
        ]}
        value={tab}
      />
      <div aria-labelledby={`admin-security-tab-${tab}`} className="admin-tabpanel" id="admin-security-panel" role="tabpanel">
        {summary.error && !summary.data ? <LoadFailure message={summary.error} onRetry={summary.reload} /> : null}
        {tab === 'events' ? (
          <>
            {summary.data ? <SecurityFigures summary={summary.data} /> : !summary.error ? <Skeleton lines={3} /> : null}
            {summary.data ? <SecurityCharts range={props.range} summary={summary.data} /> : null}
            <div className="admin-grid is-wide-left">
              <EventTable {...props} />
              {summary.data ? (
                <ChartCard
                  subtitle={translate('admin.environmentsSubtitle', { count: formatCount(summary.data.activeSessions) })}
                  table={{ columns: [translate('admin.browserAndSystem'), translate('admin.sessions')], rows: summary.data.environments.map((item) => [item.label ?? translate('admin.unknownBrowser'), item.count]) }}
                  title={translate('admin.environments')}
                >
                  <BarList
                    items={environmentItems(summary.data)}
                    label={translate('admin.environments')}
                  />
                </ChartCard>
              ) : null}
            </div>
          </>
        ) : tab === 'sessions' ? (
          <Sessions {...props} />
        ) : tab === 'actions' ? (
          <Actions {...props} />
        ) : summary.data ? (
          <Limits summary={summary.data} />
        ) : null}
      </div>
    </div>
  );
}

function environmentItems(summary: AdminSecuritySummary) {
  // The five most common environments; the rest folded into "Other".
  const top = summary.environments.slice(0, 5);
  const rest = summary.environments.slice(5).reduce((sum, item) => sum + item.count, 0);
  return [
    ...top.map((item, index) => ({ key: `${item.label ?? 'unknown'}-${index}`, label: item.label ?? translate('admin.unknownBrowser'), value: item.count })),
    ...(rest ? [{ key: 'other', label: translate('admin.other'), value: rest }] : []),
  ];
}

function SecurityFigures({ summary }: { summary: AdminSecuritySummary }) {
  const range = rangeLabel(summary.range);
  const failures = summary.loginFailures;
  const limits = summary.rateLimited;
  return (
    <FigureBand label={translate('admin.securityFigures')}>
      <Figure detail={translate('admin.mfaLogins', { count: formatCount(summary.mfaLogins) })} label={translate('admin.loginsIn', { range })} value={formatCount(summary.logins)} />
      <Figure
        detail={translate('admin.factorsDetail', { authenticator: formatCount(summary.factors.authenticator), passkey: formatCount(summary.factors.passkey) })}
        label={translate('admin.twoFactorCoverage')}
        value={formatPercent(summary.factors.twoFactor, summary.factors.users)}
      />
      <Figure detail={translate('admin.trustedBrowsersCount', { count: formatCount(summary.trustedBrowsers) })} label={translate('admin.activeSessions')} value={formatCount(summary.activeSessions)} />
      {failures !== undefined ? (
        <Figure
          detail={failures ? peakDay(failures.daily) : translate('admin.notRecordedYet')}
          label={translate('admin.loginFailuresIn', { range })}
          value={failures ? formatCount(failures.total) : '—'}
        />
      ) : null}
      {limits !== undefined ? (
        <Figure
          detail={limits ? Object.entries(limits.byKind).map(([kind, count]) => `${limitKindLabel(kind)} ${formatCount(count)}`).join(' · ') || translate('admin.none') : translate('admin.notRecordedYet')}
          label={translate('admin.rateLimitedIn', { range })}
          value={limits ? formatCount(limits.total) : '—'}
        />
      ) : null}
    </FigureBand>
  );
}

function peakDay(days: Array<{ day: string; count: number }>) {
  const peak = days.reduce<{ day: string; count: number } | null>((best, day) => (!best || day.count > best.count ? day : best), null);
  return peak && peak.count ? translate('admin.peakDay', { day: formatDay(peak.day), count: formatCount(peak.count) }) : translate('admin.none');
}

export function limitKindLabel(kind: string) {
  switch (kind) {
    case 'login': return translate('admin.limitLogin');
    case 'auth-ip': return translate('admin.limitAuthIp');
    case 'enroll': return translate('admin.limitEnroll');
    case 'factor': return translate('admin.limitChallenge');
    case 'reauth': return translate('admin.limitReauth');
    case 'vault': return translate('admin.limitVault');
    case 'passkey': return translate('admin.limitPasskey');
    case 'recover': return translate('admin.limitRecover');
    default: return kind;
  }
}

function SecurityCharts({ summary, range }: { summary: AdminSecuritySummary; range: AdminPageProps['range'] }) {
  const categories = CHART_CATEGORIES.filter((category) => summary.daily.some((day) => day.categories[category]));
  const shown = categories.length ? categories : [...CHART_CATEGORIES];
  const failures = summary.loginFailures;
  const rangeStart = Date.now() - ({ '24h': 1, '7d': 7, '30d': 30, '90d': 90 }[range]) * 86_400_000;
  return (
    <div className={failures !== undefined ? 'admin-grid is-wide-left' : 'admin-grid'}>
      <ChartCard
        subtitle={translate('admin.eventsPerDay', { days: summary.daily.length })}
        table={{ columns: [translate('admin.day'), ...shown.map(eventCategoryLabel)], rows: summary.daily.map((day) => [formatDay(day.day), ...shown.map((category) => day.categories[category] ?? 0)]) }}
        title={translate('admin.securityEvents')}
        footer={summary.history.oldestEventAt ? translate('admin.auditKeeps', { count: formatCount(summary.history.events), time: formatDateTime(summary.history.oldestEventAt) }) : null}
      >
        <ColumnChart
          data={summary.daily.map((day) => ({ key: day.day, label: formatDay(day.day), values: day.categories as Record<string, number> }))}
          height={200}
          label={translate('admin.securityEvents')}
          series={shown.map((category) => ({ key: category, label: eventCategoryLabel(category), slot: CATEGORY_SLOTS[category] ?? 1 }))}
        />
        <Legend items={shown.map((category) => ({ label: eventCategoryLabel(category), slot: CATEGORY_SLOTS[category] ?? 1 }))} />
      </ChartCard>
      {failures !== undefined ? (
        <ChartCard
          subtitle={translate('admin.eventsPerDay', { days: failures?.daily.length ?? 0 })}
          table={failures ? { columns: [translate('admin.day'), translate('admin.loginFailures')], rows: failures.daily.map((day) => [formatDay(day.day), day.count]) } : null}
          title={translate('admin.loginFailures')}
        >
          {failures ? (
            <>
              <LineChart
                formatAt={formatDay}
                label={translate('admin.loginFailures')}
                points={failures.daily.map((day) => ({ at: Date.parse(`${day.day}T12:00:00Z`), value: day.count }))}
                slot={2}
              />
              <HistoryNote historyStart={failures.historyStart} rangeStart={rangeStart} />
            </>
          ) : <p className="admin-empty">{translate('admin.notRecordedYet')}</p>}
        </ChartCard>
      ) : null}
    </div>
  );
}

function eventFilterLabel(filter: (typeof EVENT_FILTERS)[number]) {
  return filter === 'all' ? translate('admin.filterAll') : eventCategoryLabel(filter);
}

function EventTable(props: AdminPageProps) {
  useI18n();
  const filter = (EVENT_FILTERS as readonly string[]).includes(props.filter ?? '') ? (props.filter as (typeof EVENT_FILTERS)[number]) : 'all';
  const [page, setPage] = useState(1);
  useEffect(() => setPage(1), [filter, props.range]);
  const { data, error, loading, reload } = useAdminResource(
    () => adminApi.securityEvents({ type: filter, range: props.range, page, pageSize: 15 }),
    `${filter}:${props.range}:${page}:${props.refreshKey}`,
    props.onUnauthorized,
  );
  const columns: Array<Column<AdminSecurityEvent>> = [
    { key: 'time', header: translate('admin.time'), render: (event) => <time dateTime={event.at} title={formatDateTime(event.at)}>{formatDateTime(event.at)}</time> },
    { key: 'event', header: translate('admin.event'), render: (event) => <span className="admin-event-cell"><EventIcon category={event.category} event={event.event} />{eventLabel(event.event)}</span> },
    { key: 'user', header: translate('admin.user'), render: (event) => (event.userId ? <button className="admin-text-link" onClick={() => props.go('users', { id: event.userId })} type="button">{event.username ?? event.userId}</button> : '—') },
    { key: 'resource', header: translate('admin.resource'), render: (event) => event.resourceLabel ?? (event.resourceId ? <span className="admin-mono admin-truncate">{event.resourceId}</span> : '—') },
  ];
  return (
    <Card subtitle={translate('admin.eventDetailsSubtitle')} title={translate('admin.eventDetails')}>
      <FilterChips
        label={translate('admin.filterEvents')}
        onChange={(key) => props.setParams({ filter: key === 'all' ? null : key })}
        options={EVENT_FILTERS.map((key) => ({ key, label: eventFilterLabel(key) }))}
        value={filter}
      />
      {error && !data ? <LoadFailure message={error} onRetry={reload} /> : null}
      {data ? (
        <>
          <DataTable
            busy={loading}
            columns={columns}
            empty={translate('admin.noEvents')}
            label={translate('admin.eventDetails')}
            phoneRow={(event) => (
              <>
                <EventIcon category={event.category} event={event.event} />
                <span className="admin-two-line">
                  <b>{event.username ? `${event.username} · ${eventLabel(event.event)}` : eventLabel(event.event)}</b>
                  <span>{formatRelative(event.at)}{event.resourceLabel ? ` · ${event.resourceLabel}` : ''}</span>
                </span>
              </>
            )}
            rowKey={(event) => String(event.id)}
            rows={data.items}
          />
          <Pagination onPage={setPage} page={data.page} pageSize={data.pageSize} total={data.total} />
        </>
      ) : !error ? <Skeleton lines={6} /> : null}
    </Card>
  );
}

function Sessions(props: AdminPageProps) {
  useI18n();
  const [text, setText] = useState('');
  const q = useDebounced(text.trim());
  const [page, setPage] = useState(1);
  useEffect(() => setPage(1), [q]);
  const { data, error, loading, reload } = useAdminResource(
    () => adminApi.sessions({ q, page, pageSize: 25 }),
    `${q}:${page}:${props.refreshKey}`,
    props.onUnauthorized,
  );
  const [failure, setFailure] = useState<{ id: string; message: string } | null>(null);
  const end = async (session: AdminSession) => {
    const confirmed = await confirmAsync({
      title: translate('admin.endSessionTitle'),
      description: translate('admin.endSessionOfBody', { name: session.username ?? '', environment: session.environment ?? translate('admin.unknownBrowser') }),
      confirmLabel: translate('admin.endSession'),
    });
    if (!confirmed) return;
    setFailure(null);
    try {
      await adminApi.endSession(session.id);
      reload();
    } catch (caught) {
      setFailure({ id: session.id, message: errorText(caught) });
    }
  };
  const columns: Array<Column<AdminSession>> = [
    { key: 'user', header: translate('admin.user'), render: (session) => <button className="admin-text-link" onClick={() => props.go('users', { id: session.userId ?? null })} type="button">{session.username}</button> },
    { key: 'environment', header: translate('admin.browserAndSystem'), render: (session) => session.environment ?? <span className="admin-muted">{translate('admin.unknownBrowser')}</span> },
    { key: 'created', header: translate('admin.signedIn'), render: (session) => <time title={formatDateTime(session.createdAt)}>{formatRelative(session.createdAt)}</time> },
    { key: 'expires', header: translate('admin.expires'), render: (session) => formatRelative(session.expiresAt) },
    {
      key: 'trust',
      header: translate('admin.verification'),
      render: (session) => (
        <span className="admin-pills">
          {session.strongAuthAt ? <Badge icon={<ShieldCheck aria-hidden className="h-3.5 w-3.5" />} tone="success">{translate('admin.verified')}</Badge> : null}
          {session.trustedBrowser ? <Badge tone="neutral">{translate('admin.trustedBrowser')}</Badge> : null}
        </span>
      ),
    },
    {
      key: 'end',
      header: <span className="sr-only">{translate('admin.actions')}</span>,
      className: 'is-menu',
      render: (session) => (
        <span className="admin-inline">
          <button className="admin-text-button is-danger" disabled={session.role === 'admin'} onClick={() => void end(session)} type="button">{translate('admin.endSession')}</button>
          {failure?.id === session.id ? <ErrorMark message={failure.message} /> : null}
        </span>
      ),
    },
  ];
  return (
    <>
      <div className="admin-toolbar">
        <SearchField label={translate('admin.searchSessions')} onChange={setText} placeholder={translate('admin.searchSessions')} value={text} />
      </div>
      {error && !data ? <LoadFailure message={error} onRetry={reload} /> : null}
      {data ? (
        <Card className="admin-table-card">
          <DataTable
            busy={loading}
            columns={columns}
            empty={translate('admin.noSessions')}
            label={translate('admin.sessions')}
            phoneRow={(session) => (
              <>
                <span className="admin-two-line">
                  <b>{session.username} · {session.environment ?? translate('admin.unknownBrowser')}</b>
                  <span>{translate('admin.signedInAt', { time: formatRelative(session.createdAt) })}</span>
                </span>
                <button className="admin-text-button is-danger" disabled={session.role === 'admin'} onClick={() => void end(session)} type="button">{translate('admin.endSession')}</button>
              </>
            )}
            rowKey={(session) => session.id}
            rows={data.items}
          />
          <Pagination onPage={setPage} page={data.page} pageSize={data.pageSize} total={data.total} />
        </Card>
      ) : !error ? <Skeleton lines={6} /> : null}
    </>
  );
}

function Actions(props: AdminPageProps) {
  useI18n();
  const [page, setPage] = useState(1);
  const { data, error, loading, reload } = useAdminResource(() => adminApi.actions({ page, pageSize: 25 }), `${page}:${props.refreshKey}`, props.onUnauthorized);
  const columns: Array<Column<AdminAction>> = [
    { key: 'time', header: translate('admin.time'), render: (action) => formatDateTime(action.at) },
    { key: 'action', header: translate('admin.action'), render: (action) => <span className="admin-event-cell"><EventIcon category="activity" event={action.action} />{actionLabel(action.action)}</span> },
    { key: 'target', header: translate('admin.target'), render: (action) => action.targetLabel ?? '—' },
    { key: 'admin', header: translate('admin.admin'), render: (action) => action.adminUsername },
    { key: 'detail', header: translate('admin.detail'), render: (action) => actionDetail(action) },
  ];
  return (
    <Card subtitle={translate('admin.adminActionsLogSubtitle')} title={translate('admin.adminActions')}>
      {error && !data ? <LoadFailure message={error} onRetry={reload} /> : null}
      {data ? (
        <>
          <DataTable
            busy={loading}
            columns={columns}
            empty={translate('admin.noAdminActions')}
            label={translate('admin.adminActions')}
            phoneRow={(action) => (
              <>
                <EventIcon category="activity" event={action.action} />
                <span className="admin-two-line">
                  <b>{action.targetLabel ? `${actionLabel(action.action)} · ${action.targetLabel}` : actionLabel(action.action)}</b>
                  <span>{action.adminUsername} · {formatRelative(action.at)}</span>
                </span>
              </>
            )}
            rowKey={(action) => String(action.id)}
            rows={data.items}
          />
          <Pagination onPage={setPage} page={data.page} pageSize={data.pageSize} total={data.total} />
        </>
      ) : !error ? <Skeleton lines={6} /> : null}
    </Card>
  );
}

/** A short readable form of an action's recorded detail. */
export function actionDetail(action: AdminAction) {
  const detail = action.detail;
  if (!detail) return '—';
  const parts: string[] = [];
  for (const [key, value] of Object.entries(detail)) {
    if (Array.isArray(value) && value.length === 2) parts.push(`${key}: ${String(value[0])} → ${String(value[1])}`);
    else if (value !== null && typeof value !== 'object') parts.push(`${key}: ${String(value)}`);
  }
  return parts.join(' · ') || '—';
}

function Limits({ summary }: { summary: AdminSecuritySummary }) {
  useI18n();
  const limits = summary.rateLimited;
  if (!limits) return <p className="admin-empty">{translate('admin.notRecordedYet')}</p>;
  return (
    <div className="admin-grid is-halves">
      <ChartCard
        subtitle={translate('admin.limitsSubtitle')}
        table={{ columns: [translate('admin.limitKind'), translate('admin.refusals')], rows: Object.entries(limits.byKind).map(([kind, count]) => [limitKindLabel(kind), count]) }}
        title={translate('admin.rateLimits')}
      >
        {Object.keys(limits.byKind).length ? (
          <BarList items={Object.entries(limits.byKind).map(([kind, count]) => ({ key: kind, label: limitKindLabel(kind), value: count }))} label={translate('admin.rateLimits')} />
        ) : <p className="admin-empty">{translate('admin.noRefusals')}</p>}
      </ChartCard>
      <ChartCard
        subtitle={translate('admin.eventsPerDay', { days: limits.daily.length })}
        table={{ columns: [translate('admin.day'), translate('admin.refusals')], rows: limits.daily.map((day) => [formatDay(day.day), day.count]) }}
        title={translate('admin.refusalsPerDay')}
      >
        <ColumnChart
          data={limits.daily.map((day) => ({ key: day.day, label: formatDay(day.day), values: { count: day.count } }))}
          height={180}
          label={translate('admin.refusalsPerDay')}
          series={[{ key: 'count', label: translate('admin.refusals'), slot: 2 }]}
        />
      </ChartCard>
    </div>
  );
}
