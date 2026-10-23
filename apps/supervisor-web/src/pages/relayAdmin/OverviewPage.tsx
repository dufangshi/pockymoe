// Overview: what needs an admin, the key figures, activity and sign-up
// trends, device versions and tunnels (when the relay records them), and the
// latest security events and admin actions.
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import {
  Activity,
  ArrowRight,
  CircleCheck,
  KeyRound,
  Link2Off,
  MonitorSmartphone,
  Share2,
  UserPlus,
  Users,
} from 'lucide-react';
import { useEffect, useMemo } from 'react';
import { adminApi } from './adminApi';
import { pageWindow, windowLabel } from './ranges';
import { BarList, ChartCard, ColumnChart, HistoryNote, LineChart, ProportionBar, Sparkline, type SeriesSlot } from './charts';
import { formatCount, formatDateTime, formatDay, formatDuration, formatPercent, formatRatio, formatRelative, formatTime } from './format';
import { rangeLabel } from './labels';
import type { AdminPageProps } from './RelayAdminConsole';
import type { AdminOverview, AdminSeries } from './types';
import { Badge, Card, Figure, FigureBand, LoadFailure, MiniStat, Skeleton, useAdminResource } from './ui';
import { ActionList, EventList } from './events';

export function OverviewPage(props: AdminPageProps) {
  useI18n();
  const { range, customRange, refreshKey, onLoaded, onUnauthorized, go } = props;
  // One range for the page: every chart over time follows it (and the URL).
  const request = useMemo(() => pageWindow({ range, customRange }), [range, customRange]);
  const { data, error, loading, reload } = useAdminResource(() => adminApi.overview(request), `${JSON.stringify(request)}:${refreshKey}`, onUnauthorized);
  useEffect(() => {
    if (data) onLoaded();
  }, [data, onLoaded]);
  if (!data) return error ? <LoadFailure message={error} onRetry={reload} /> : <Skeleton lines={6} />;
  return (
    <div aria-busy={loading || undefined} className={`admin-page admin-overview${loading ? ' is-busy' : ''}`}>
      {error ? <LoadFailure message={error} onRetry={reload} /> : null}
      <Attention data={data} go={go} />
      <KeyFigures data={data} />
      <div className="admin-grid is-wide-left">
        <OnlineDevicesCard data={data} />
        <ChartCard
          aside={<NowBadge />}
          subtitle={translate('admin.recentActivitySubtitle')}
          table={{
            columns: [translate('admin.lastActive'), translate('admin.people')],
            rows: data.activity.map((bucket) => [activityLabel(bucket.key), bucket.count]),
          }}
          title={translate('admin.recentActivity')}
          footer={translate('admin.recentActivityFooter', { total: formatCount(data.users.total) })}
        >
          <BarList
            items={data.activity.map((bucket) => ({ key: bucket.key, label: activityLabel(bucket.key), value: bucket.count }))}
            label={translate('admin.recentActivity')}
          />
        </ChartCard>
      </div>
      <div className="admin-grid is-thirds">
        <SignupsCard data={data} />
        <VersionsCard data={data} go={go} />
        <TunnelsCard data={data} go={go} />
      </div>
      <div className="admin-grid is-halves">
        <Card
          aside={
            <button className="admin-link-button" onClick={() => go('security')} type="button">
              {translate('admin.all')}
              <ArrowRight aria-hidden className="h-4 w-4" />
            </button>
          }
          subtitle={translate('admin.securityEventsSubtitle')}
          title={translate('admin.recentSecurityEvents')}
        >
          <EventList events={data.securityEvents} links={{ onOpenDevice: (id) => go('devices', { id }), onOpenUser: (id) => go('users', { id }) }} />
        </Card>
        {data.adminActions !== undefined ? (
          <Card
            aside={
              <button className="admin-link-button" onClick={() => go('security', { sub: 'actions' })} type="button">
                {translate('admin.all')}
                <ArrowRight aria-hidden className="h-4 w-4" />
              </button>
            }
            subtitle={translate('admin.adminActionsSubtitle')}
            title={translate('admin.adminActions')}
          >
            <ActionList actions={data.adminActions ?? []} links={{ onOpenDevice: (id) => go('devices', { id }), onOpenUser: (id) => go('users', { id }) }} />
          </Card>
        ) : null}
      </div>
    </div>
  );
}

/** Marks a card that shows the current state, not the page's range. */
function NowBadge() {
  useI18n();
  return <Badge tone="neutral">{translate('admin.asOfNow')}</Badge>;
}

/** The page's window: start and end, and how to label times in it. */
function chartSpan(data: AdminOverview) {
  const start = Date.parse(data.window?.start ?? data.rangeStart);
  const end = Date.parse(data.window?.end ?? data.generatedAt);
  const short = end - start <= 2 * 86_400_000;
  return { start, end, short, formatAt: (at: number) => (short ? formatTime(at) : formatDay(at)), label: windowLabel(data) };
}

function SignupsCard({ data }: { data: AdminOverview }) {
  useI18n();
  const footer = (
    <>
      <span>{translate('admin.medianApproval', { time: data.registrations.medianApprovalHours === null ? '—' : formatDuration(data.registrations.medianApprovalHours * 3600) })}</span>
      <span>{translate('admin.approvalRate', { rate: formatPercent(data.registrations.approved30d, data.registrations.approved30d + data.registrations.rejected30d) })}</span>
    </>
  );
  // Older relays answer with 12 weeks only.
  if (!data.signups) {
    return (
      <ChartCard
        footer={footer}
        subtitle={translate('admin.signupsSubtitle', { total: formatCount(data.signupsWeekly.reduce((sum, week) => sum + week.count, 0)) })}
        table={{ columns: [translate('admin.weekOf'), translate('admin.newAccounts')], rows: data.signupsWeekly.map((week) => [formatDay(week.weekStart), week.count]) }}
        title={translate('admin.signups')}
      >
        <ColumnChart
          data={data.signupsWeekly.map((week) => ({ key: week.weekStart, label: formatDay(week.weekStart), values: { count: week.count } }))}
          height={180}
          label={translate('admin.signups')}
          series={[{ key: 'count', label: translate('admin.newAccounts'), slot: 1 }]}
        />
      </ChartCard>
    );
  }
  const signups = data.signups;
  const step = signups.resolutionSeconds;
  const formatBucket = (at: number) => (step < 86_400 ? formatTime(at) : formatDay(at));
  const unit = step < 86_400 ? translate('admin.perHour') : step < 7 * 86_400 ? translate('admin.perDay') : translate('admin.perWeek');
  return (
    <ChartCard
      footer={footer}
      subtitle={translate('admin.signupsInRange', { unit, total: formatCount(signups.total), range: chartSpan(data).label })}
      table={{ columns: [translate('admin.time'), translate('admin.newAccounts')], rows: signups.points.map(([at, count]) => [step < 86_400 ? formatDateTime(at) : formatDay(at), count]) }}
      title={translate('admin.signups')}
    >
      <ColumnChart
        data={signups.points.map(([at, count]) => ({ key: String(at), label: formatBucket(at), values: { count } }))}
        height={180}
        label={translate('admin.signups')}
        series={[{ key: 'count', label: translate('admin.newAccounts'), slot: 1 }]}
      />
    </ChartCard>
  );
}

function activityLabel(key: AdminOverview['activity'][number]['key']) {
  switch (key) {
    case '1h': return translate('admin.activeHour');
    case '24h': return translate('admin.activeDay');
    case '7d': return translate('admin.activeWeek');
    case '30d': return translate('admin.activeMonth');
    case 'older': return translate('admin.activeOlder');
    case 'never': return translate('admin.neverSignedIn');
  }
}

function Attention({ data, go }: { data: AdminOverview; go: AdminPageProps['go'] }) {
  const items = [
    data.attention.pendingRegistrations > 0 && {
      key: 'registrations',
      icon: <UserPlus aria-hidden className="h-5 w-5" />,
      value: data.attention.pendingRegistrations,
      text: translate('admin.attentionRegistrations'),
      detail: data.attention.oldestPendingAt ? translate('admin.oldestWaiting', { time: formatRelative(data.attention.oldestPendingAt) }) : null,
      action: translate('admin.review'),
      onGo: () => go('users', { sub: 'registrations' }),
    },
    (data.attention.unstableDevices ?? 0) > 0 && {
      key: 'unstable',
      icon: <Activity aria-hidden className="h-5 w-5" />,
      value: data.attention.unstableDevices ?? 0,
      text: translate('admin.attentionUnstable'),
      detail: null,
      action: translate('admin.viewDevices'),
      onGo: () => go('devices', { params: { state: 'unstable' } }),
    },
    data.attention.usersWithoutAccountKey > 0 && {
      key: 'keys',
      icon: <KeyRound aria-hidden className="h-5 w-5" />,
      value: data.attention.usersWithoutAccountKey,
      text: translate('admin.attentionNoKey'),
      detail: null,
      action: translate('admin.viewList'),
      onGo: () => go('users', { params: { key: 'missing' } }),
    },
    data.attention.legacyPublicLinks > 0 && {
      key: 'links',
      icon: <Link2Off aria-hidden className="h-5 w-5" />,
      value: data.attention.legacyPublicLinks,
      text: translate('admin.attentionLegacyLinks'),
      detail: null,
      action: translate('admin.viewLinks'),
      onGo: () => go('shares', { sub: 'links', filter: 'legacy' }),
    },
  ].filter(Boolean) as Array<{ key: string; icon: React.ReactNode; value: number; text: string; detail: string | null; action: string; onGo: () => void }>;
  return (
    <Card className="admin-attention" subtitle={translate('admin.attentionSubtitle')} title={translate('admin.attention')}>
      {items.length ? (
        <ul className="admin-attention-list">
          {items.map((item) => (
            <li key={item.key}>
              <button className="admin-attention-item" onClick={item.onGo} type="button">
                <span className="admin-attention-icon">{item.icon}</span>
                <span className="admin-attention-text">
                  <b>{formatCount(item.value)}</b>
                  <span>{item.text}{item.detail ? ` · ${item.detail}` : ''}</span>
                  <span className="admin-attention-action">
                    {item.action}
                    <ArrowRight aria-hidden className="h-3.5 w-3.5" />
                  </span>
                </span>
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="admin-all-clear">
          <CircleCheck aria-hidden className="h-5 w-5" />
          {translate('admin.nothingNeedsAttention')}
        </p>
      )}
    </Card>
  );
}

function KeyFigures({ data }: { data: AdminOverview }) {
  const users = data.users;
  return (
    <FigureBand label={translate('admin.keyFigures')}>
      <Figure
        detail={data.window
          ? translate('admin.devicesFigureDetailIn', { added: formatCount(data.devices.newInRange), unprotected: formatCount(data.devices.unprotected), range: chartSpan(data).label })
          : translate('admin.devicesFigureDetail', { added: formatCount(data.devices.newThisWeek), unprotected: formatCount(data.devices.unprotected) })}
        icon={<MonitorSmartphone className="h-4 w-4" />}
        label={translate('admin.onlineDevices')}
        total={formatCount(data.devices.total)}
        value={formatCount(data.devices.online)}
      />
      <Figure
        detail={translate('admin.activeFigureDetail', { day: formatCount(users.active24h), month: formatCount(users.active30d) })}
        icon={<Users className="h-4 w-4" />}
        label={translate('admin.activeUsers7d')}
        total={formatCount(users.total)}
        trend={users.dailyActive?.length ? <Sparkline label={translate('admin.dailyActiveUsers')} values={users.dailyActive.map((day) => day.users)} width={72} /> : null}
        value={formatCount(users.active7d)}
      />
      <Figure
        detail={data.shares.createdInRange !== undefined
          ? translate('admin.sharesFigureDetailIn', { added: formatCount(data.shares.createdInRange), revoked: formatCount(data.shares.revokedInRange ?? 0), expiring: formatCount(data.shares.expiringSoon), range: chartSpan(data).label })
          : translate('admin.sharesFigureDetail', { added: formatCount(data.shares.createdThisWeek), revoked: formatCount(data.shares.revokedThisWeek), expiring: formatCount(data.shares.expiringSoon) })}
        icon={<Share2 className="h-4 w-4" />}
        label={translate('admin.activeShares')}
        value={formatCount(data.shares.active)}
      />
      <Figure
        detail={translate('admin.keysFigureDetail', { devices: formatPercent(data.devices.protected, data.devices.total), factors: formatPercent(users.twoFactor, users.total) })}
        icon={<KeyRound className="h-4 w-4" />}
        label={translate('admin.accountKeyCoverage')}
        total={`${formatCount(users.withAccountKey)} / ${formatCount(users.total)}`}
        value={formatPercent(users.withAccountKey, users.total)}
      />
      <Figure
        detail={translate('admin.registrationsFigureDetail', { approved: formatCount(data.registrations.approved30d), rejected: formatCount(data.registrations.rejected30d) })}
        icon={<UserPlus className="h-4 w-4" />}
        label={translate('admin.pendingRegistrations')}
        value={formatCount(data.registrations.pending)}
      />
    </FigureBand>
  );
}

export function seriesPoints(series: AdminSeries) {
  return series.points.map(([at, value]) => ({ at, value }));
}

function OnlineDevicesCard({ data }: { data: AdminOverview }) {
  if (data.onlineDevices === undefined) return null;
  const series = data.onlineDevices;
  const { start: rangeStart, end, formatAt } = chartSpan(data);
  return (
    <ChartCard
      subtitle={series ? translate('admin.onlineDevicesSubtitle', { minutes: Math.round(series.resolutionSeconds / 60) }) : translate('admin.notRecordedYet')}
      table={series ? { columns: [translate('admin.time'), translate('admin.onlineDevices')], rows: series.points.filter(([, value]) => value !== null).map(([at, value]) => [formatDateTime(at), value ?? 0]) } : null}
      title={translate('admin.onlineDevices')}
    >
      {series ? (
        <>
          <LineChart end={end} formatAt={formatAt} label={translate('admin.onlineDevices')} points={seriesPoints(series)} start={rangeStart} />
          <HistoryNote historyStart={series.historyStart} rangeStart={rangeStart} />
        </>
      ) : (
        <p className="admin-empty">{translate('admin.notRecordedYet')}</p>
      )}
    </ChartCard>
  );
}

const VERSION_SLOTS: SeriesSlot[] = [1, 2, 3, 4, 5];

function VersionsCard({ data, go }: { data: AdminOverview; go: AdminPageProps['go'] }) {
  if (data.versions === undefined) return null;
  const versions = data.versions ?? [];
  const online = versions.reduce((sum, item) => sum + item.online, 0);
  const onlineWhere = (test: (item: (typeof versions)[number]) => boolean) => versions.filter(test).reduce((sum, item) => sum + item.online, 0);
  const olderDevices = versions.filter((item) => item.outdated).reduce((sum, item) => sum + item.devices, 0);
  // Five slots: the four most common versions, the rest folded into "older".
  const top = versions.filter((item) => item.version).slice(0, 4);
  const rest = versions.filter((item) => !top.includes(item)).reduce((sum, item) => sum + item.online, 0);
  const items = [
    ...top.map((item, index) => ({ key: item.version!, label: item.version!, value: item.online, slot: VERSION_SLOTS[index]! })),
    ...(rest ? [{ key: 'rest', label: translate('admin.otherVersions'), value: rest, slot: 5 as SeriesSlot }] : []),
  ];
  return (
    <ChartCard
      aside={<NowBadge />}
      subtitle={translate('admin.versionsSubtitle', { online: formatCount(online), latest: data.relay.version })}
      table={{ columns: [translate('admin.version'), translate('admin.onlineDevices'), translate('admin.allDevices')], rows: versions.map((item) => [item.version ?? translate('admin.unknown'), item.online, item.devices]) }}
      title={translate('admin.deviceVersions')}
      footer={
        olderDevices ? (
          <button className="admin-link-button" onClick={() => go('devices', { params: { version: 'outdated' } })} type="button">
            {translate('admin.viewOlderDevices', { count: olderDevices })}
            <ArrowRight aria-hidden className="h-4 w-4" />
          </button>
        ) : null
      }
    >
      {online ? (
        <>
          <ProportionBar items={items} label={translate('admin.deviceVersions')} />
          <div className="admin-mini-stats">
            <MiniStat label={translate('admin.onRelayVersion')} value={formatCount(onlineWhere((item) => item.version === data.relay.version))} />
            <MiniStat label={translate('admin.onOlderVersion')} value={formatCount(onlineWhere((item) => Boolean(item.outdated)))} />
            <MiniStat label={translate('admin.versionNotReported')} value={formatCount(onlineWhere((item) => item.version === null))} />
          </div>
        </>
      ) : <p className="admin-empty">{translate('admin.noVersionsYet')}</p>}
    </ChartCard>
  );
}

/** A latency in ms; at the histogram's open bucket it is a floor ("≥ 30,000 ms"). */
export function formatLatency(value: number | null | undefined, openAboveMs?: number) {
  if (value === null || value === undefined || !Number.isFinite(value)) return '—';
  const rounded = value >= 100 ? Math.round(value / 5) * 5 : Math.round(value);
  const text = translate('admin.milliseconds', { value: formatCount(rounded) });
  return openAboveMs !== undefined && value >= openAboveMs ? `≥ ${text}` : text;
}

function TunnelsCard({ data, go }: { data: AdminOverview; go: AdminPageProps['go'] }) {
  if (data.tunnels === undefined) return null;
  const tunnels = data.tunnels;
  const details = (
    <button className="admin-link-button" onClick={() => go('performance')} type="button">
      {translate('admin.details')}
      <ArrowRight aria-hidden className="h-4 w-4" />
    </button>
  );
  if (!tunnels) {
    return (
      <ChartCard aside={details} title={translate('admin.tunnels')}>
        <p className="admin-empty">{translate('admin.notRecordedYet')}</p>
      </ChartCard>
    );
  }
  const values = (series: AdminSeries) => series.points.map(([, value]) => value);
  const open = tunnels.latencyBoundsMs?.[tunnels.latencyBoundsMs.length - 1];
  return (
    <ChartCard aside={details} subtitle={translate('admin.tunnelsSubtitle', { range: chartSpan(data).label })} title={translate('admin.tunnels')}>
      <ul className="admin-tunnel-stats">
        <li>
          <span>{translate('admin.reconnects')}</span>
          <b>{formatCount(tunnels.reconnects)}</b>
          <small>{translate('admin.unstableCount', { count: formatCount(tunnels.unstableDevices) })}</small>
          <Sparkline label={translate('admin.reconnects')} slot={1} values={values(tunnels.reconnectSeries)} />
        </li>
        <li>
          <span>{translate('admin.forwardErrorRate')}</span>
          <b>{formatRatio(tunnels.errorRate)}</b>
          <small>{translate('admin.requestsCount', { count: formatCount(tunnels.requests) })}</small>
          <Sparkline label={translate('admin.forwardErrorRate')} slot={2} values={values(tunnels.errorSeries)} />
        </li>
        <li>
          <span>{translate('admin.forwardP95')}</span>
          <b>{formatLatency(tunnels.p95Ms, open)}</b>
          <small>{tunnels.latencyBoundsMs ? translate('admin.forwardLatencyEstimated') : translate('admin.forwardLatencyNote')}</small>
          <Sparkline label={translate('admin.forwardP95')} slot={3} values={values(tunnels.latencySeries)} />
        </li>
      </ul>
    </ChartCard>
  );
}
