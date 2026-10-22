// System: the relay process, configuration checks, push delivery, device
// versions and storage.
import { PathChip } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { CircleCheck, CircleDashed, TriangleAlert } from 'lucide-react';
import { useEffect } from 'react';
import { adminApi } from './adminApi';
import { BarList, ChartCard, HistoryNote, LineChart, ProportionBar, type SeriesSlot } from './charts';
import { formatBytes, formatCount, formatDay, formatDuration, formatRatio, formatRelative } from './format';
import { checkLabel } from './labels';
import { seriesPoints } from './OverviewPage';
import type { AdminPageProps } from './RelayAdminConsole';
import type { AdminSystem, AdminSystemCheck } from './types';
import { Badge, Card, Facts, LoadFailure, MiniStat, Skeleton, useAdminResource } from './ui';

export function SystemPage(props: AdminPageProps) {
  useI18n();
  const { data, error, loading, reload } = useAdminResource(() => adminApi.system(), `system:${props.refreshKey}`, props.onUnauthorized);
  const loaded = props.onLoaded;
  useEffect(() => {
    if (data) loaded();
  }, [data, loaded]);
  if (!data) return error ? <LoadFailure message={error} onRetry={reload} /> : <Skeleton lines={8} />;
  const failing = data.checks.some((check) => check.key === 'integrity' && !check.ok);
  return (
    <div aria-busy={loading || undefined} className={`admin-page${loading ? ' is-busy' : ''}`}>
      <div className="admin-grid is-thirds">
        <Card
          aside={failing ? <Badge icon={<TriangleAlert aria-hidden className="h-3.5 w-3.5" />} tone="danger">{translate('admin.needsAttention')}</Badge> : <Badge icon={<CircleCheck aria-hidden className="h-3.5 w-3.5" />} tone="success">{translate('admin.healthy')}</Badge>}
          title={translate('admin.relay')}
        >
          <Facts
            items={[
              { label: translate('admin.version'), value: <span className="admin-mono">{data.relay.version}</span> },
              { label: translate('admin.uptimeLabel'), value: formatDuration(data.relay.uptimeSeconds) },
              { label: translate('admin.publicAddress'), value: data.relay.publicBaseUrl ?? translate('admin.notConfigured') },
              { label: translate('admin.webSource'), value: data.relay.webOrigin ? translate('admin.webSeparate', { origin: data.relay.webOrigin }) : data.relay.servesWeb ? translate('admin.webServedByRelay') : translate('admin.webApiOnly') },
              data.relay.dataDir ? { label: translate('admin.dataDir'), value: <PathChip label={translate('admin.copyDataDir')} path={data.relay.dataDir} /> } : null,
            ]}
          />
          <h4 className="admin-subheading">{translate('admin.currentConnections')}</h4>
          <div className="admin-mini-stats">
            <MiniStat label={translate('admin.deviceTunnels')} value={formatCount(data.connections.deviceTunnels)} />
            <MiniStat label={translate('admin.browserSockets')} value={formatCount(data.connections.browserSockets)} />
            <MiniStat label={translate('admin.pendingRequests')} value={formatCount(data.connections.pendingRequests)} />
          </div>
        </Card>
        <Card subtitle={translate('admin.checksSubtitle')} title={translate('admin.configChecks')}>
          <ul className="admin-checks">
            {data.checks.map((check) => <Check check={check} key={check.key} />)}
          </ul>
        </Card>
        <PushCard data={data} />
      </div>
      <div className="admin-grid is-wide-left">
        <VersionsCard data={data} />
        <StorageCard data={data} />
      </div>
    </div>
  );
}

function checkDetail(check: AdminSystemCheck) {
  switch (check.key) {
    case 'integrity': return check.ok ? translate('admin.integrityOk') : String(check.detail ?? '');
    case 'sessionSecret': return check.detail === 'environment' ? translate('admin.secretFromEnvironment') : translate('admin.secretInDataDir');
    case 'publicBaseUrl': return check.detail ? String(check.detail) : translate('admin.publicBaseUrlMissing');
    case 'webOrigin': return check.detail === 'relay' ? translate('admin.webServedByRelay') : check.detail ? translate('admin.webSeparate', { origin: String(check.detail) }) : translate('admin.webApiOnly');
    case 'github':
    case 'google':
    case 'apns': return check.ok ? translate('admin.configured') : translate('admin.notConfigured');
    case 'portPreviews': return check.ok ? String(check.detail) : translate('admin.notConfigured');
    case 'legacyPublicLinks': return check.ok ? translate('admin.noLegacyLinks') : translate('admin.legacyLinksCount', { count: Number(check.detail ?? 0) });
    default: return check.detail === null ? '' : String(check.detail);
  }
}

function Check({ check }: { check: AdminSystemCheck }) {
  // Optional features that are simply off are not failures: a hollow mark.
  const optional = ['github', 'google', 'portPreviews', 'apns'].includes(check.key);
  const state = check.ok ? 'ok' : optional ? 'off' : 'warn';
  return (
    <li className={`admin-check is-${state}`}>
      {state === 'ok' ? <CircleCheck aria-hidden className="h-5 w-5" /> : state === 'off' ? <CircleDashed aria-hidden className="h-5 w-5" /> : <TriangleAlert aria-hidden className="h-5 w-5" />}
      <span className="admin-two-line">
        <b>{checkLabel(check.key)}</b>
        <span>{checkDetail(check)}</span>
      </span>
      <span className="sr-only">{state === 'ok' ? translate('admin.checkPassed') : state === 'off' ? translate('admin.notConfigured') : translate('admin.needsAttention')}</span>
    </li>
  );
}

function PushCard({ data }: { data: AdminSystem }) {
  const push = data.push;
  const turns = push.turns24h;
  const delivery = push.delivery;
  const attempts = delivery ? delivery.sent + delivery.failed + delivery.expired : 0;
  return (
    <Card subtitle={translate('admin.pushSubtitle')} title={translate('admin.pushNotifications')}>
      <div className="admin-mini-stats">
        <MiniStat label={translate('admin.subscriptions')} value={formatCount(push.subscriptions.webPush + push.subscriptions.apns)} />
        <MiniStat label={translate('admin.retrying')} value={formatCount(push.retrying)} />
        {delivery !== undefined ? (
          <MiniStat label={translate('admin.delivered')} value={delivery && attempts ? formatRatio(delivery.sent / attempts, 1) : '—'} />
        ) : (
          <MiniStat label={translate('admin.queued')} value={formatCount(push.queued)} />
        )}
      </div>
      <BarList
        items={[
          { key: 'completed', label: translate('admin.turnsCompleted'), value: turns.completed ?? 0 },
          { key: 'interrupted', label: translate('admin.turnsInterrupted'), value: turns.interrupted ?? 0 },
          { key: 'failed', label: translate('admin.turnsFailed'), value: turns.failed ?? 0 },
        ]}
        label={translate('admin.turns24h')}
      />
      <p className="admin-card-note">
        {translate('admin.subscriptionsDetail', { web: formatCount(push.subscriptions.webPush), apns: formatCount(push.subscriptions.apns) })}
        {!push.apnsConfigured ? ` · ${translate('admin.apnsOff')}` : ''}
      </p>
    </Card>
  );
}

const SLOTS: SeriesSlot[] = [1, 2, 3, 4, 5];

function VersionsCard({ data }: { data: AdminSystem }) {
  if (data.versions === undefined) return null;
  const versions = data.versions;
  if (!versions) {
    return (
      <ChartCard title={translate('admin.versionDistribution')}>
        <p className="admin-empty">{translate('admin.notRecordedYet')}</p>
      </ChartCard>
    );
  }
  const top = versions.current.filter((item) => item.version).slice(0, 4);
  const rest = versions.current.filter((item) => !top.includes(item)).reduce((sum, item) => sum + item.online, 0);
  const online = versions.current.reduce((sum, item) => sum + item.online, 0);
  const historyKeys = top.map((item) => item.version!).slice(0, 3);
  const start = versions.history[0] ? Date.parse(`${versions.history[0].day}T12:00:00Z`) : Date.now();
  return (
    <ChartCard
      subtitle={translate('admin.versionDistributionSubtitle', { online: formatCount(online) })}
      table={{
        columns: [translate('admin.version'), translate('admin.onlineDevices'), translate('admin.allDevices')],
        rows: versions.current.map((item) => [item.version ?? translate('admin.unknown'), item.online, item.devices]),
      }}
      title={translate('admin.versionDistribution')}
    >
      <ProportionBar
        items={[
          ...top.map((item, index) => ({ key: item.version!, label: item.version!, value: item.online, slot: SLOTS[index]! })),
          ...(rest ? [{ key: 'rest', label: translate('admin.otherVersions'), value: rest, slot: 5 as SeriesSlot }] : []),
        ]}
        label={translate('admin.versionDistribution')}
      />
      {versions.history.length > 1 ? (
        <div className="admin-version-history">
          {historyKeys.map((version, index) => (
            <div key={version}>
              <h4 className="admin-subheading">{translate('admin.versionOverTime', { version })}</h4>
              <LineChart
                formatAt={formatDay}
                height={120}
                label={translate('admin.versionOverTime', { version })}
                points={versions.history.map((day) => ({ at: Date.parse(`${day.day}T12:00:00Z`), value: day.versions[version] ?? 0 }))}
                slot={SLOTS[index]!}
              />
            </div>
          ))}
        </div>
      ) : null}
      <HistoryNote historyStart={versions.historyStart} rangeStart={start} />
    </ChartCard>
  );
}

function StorageCard({ data }: { data: AdminSystem }) {
  const storage = data.storage;
  const bySize = storage.tables.some((table) => table.bytes !== null);
  const tables = storage.tables.slice(0, 6);
  const rest = storage.tables.slice(6).reduce((sum, table) => sum + ((bySize ? table.bytes : table.rows) ?? 0), 0);
  const growth = storage.growth;
  return (
    <ChartCard
      subtitle={translate('admin.storageSubtitle', { database: formatBytes(storage.databaseBytes), wal: formatBytes(storage.walBytes) })}
      table={{ columns: [translate('admin.table'), bySize ? translate('admin.size') : translate('admin.rows')], rows: storage.tables.map((table) => [table.name, bySize ? formatBytes(table.bytes) : table.rows ?? 0]) }}
      title={translate('admin.storage')}
      footer={translate('admin.measuredAt', { time: formatRelative(storage.measuredAt) })}
    >
      <BarList
        formatValue={bySize ? formatBytes : formatCount}
        items={[
          ...tables.map((table) => ({ key: table.name, label: table.name.replace(/^relay_/, ''), value: (bySize ? table.bytes : table.rows) ?? 0 })),
          ...(rest ? [{ key: 'rest', label: translate('admin.other'), value: rest }] : []),
        ]}
        label={translate('admin.storage')}
      />
      {growth !== undefined ? (
        <>
          <h4 className="admin-subheading">{translate('admin.growth30d')}</h4>
          {growth ? (
            <LineChart formatAt={formatDay} formatValue={formatBytes} height={120} label={translate('admin.growth30d')} minMax={1024} points={seriesPoints(growth)} />
          ) : <p className="admin-empty">{translate('admin.notRecordedYet')}</p>}
          {growth ? <HistoryNote historyStart={growth.historyStart} rangeStart={Date.now() - 30 * 86_400_000} /> : null}
        </>
      ) : null}
    </ChartCard>
  );
}
