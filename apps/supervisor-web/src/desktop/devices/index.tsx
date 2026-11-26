import { DevicesPageTabs, UnprotectedDevicesButton } from './DevicesNavigation';
import { useAccountShareAttention } from '../../components/sharedAccess/useAccountShareAttention';
import { sharedItems } from '../../components/sharedAccess/model';
import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { Link, useLocation, useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { ArrowLeft, ArrowRight, CheckCircle2, Monitor, RefreshCw, Timer } from 'lucide-react';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { Select, SettingsPanels, useVisibleClock } from '@pockymoe/thread-ui';
import { useDesktopSelector, useDesktopStore, type DesktopDevice } from '../data';
import { DeviceManagement, DeviceStatusLine } from '../../components/DeviceManagement';
import { DeviceStateMark } from '../../components/DeviceStateMark';
import { deviceSettingsSections } from '../../components/AppShellSettingsDialog';
import { usePendingProtection } from '../../components/DeviceSetupProgress';
import { deviceActions, type DeviceManageAction } from '../../components/managed/deviceActions';
import { ManagedObjectMenu } from '../../components/managed/ManagedObjectMenu';
import { useDeviceActionHost } from '../../components/managed/useDeviceActionHost';
import { fetchAgentIsolation } from '../../lib/api';
import { devicePageHref, deviceSettingsHref, type DevicePageTab } from '../../lib/devicesRoute';
import { workspacesHref } from '../../lib/relayRoutes';
import { useMediaQuery } from '../../lib/useMediaQuery';
import { DeviceCard } from './DeviceCard';
import { HourlyActivityChart, TurnsChart, UsageChart, type ChartSource } from './Charts';
import { Info, UsagePanel } from './UsagePanel';
import { age, aggregate, aggregateHourly, calendarDays, dateKey, deviceCounts, duration, partialSummary, runtimeHours, supportsHourly, type TurnOrigin, type StatsRange } from './model';
import { useDeviceStats } from './useDeviceStats';
import './devices.css';
import { UsageColors } from './UsageColors';

const DEVICE_TABS: readonly DevicePageTab[] = ['overview', 'settings', 'access'];

export interface DevicesPageProps { onRevealDevice?(deviceId: string): void }
/**
 * The device center (issue #115). `/desktop/devices` lists the account's devices
 * with their statistics; `/desktop/devices/:id` is the one home of a device, with
 * the tabs Overview (its statistics), Settings (everything about the device) and
 * Access (who it is shared with). Computers, phones and the native hosts share it.
 */
export default function DevicesPage({ onRevealDevice }: DevicesPageProps) {
  const { t } = useI18n();
  const { portal, attention } = useAccountShareAttention();
  const index = useDesktopSelector(index => index);
  const store = useDesktopStore();
  const navigate = useNavigate();
  const route = useParams<{ id: string; deviceId: string }>();
  const [params, setParams] = useSearchParams();
  const location = useLocation();
  const narrow = useMediaQuery('(max-width: 639px)');
  const selected = route.id ?? route.deviceId ?? location.pathname.match(/^\/desktop\/devices\/([^/]+)/)?.[1] ?? params.get('device') ?? '';
  const range: StatsRange = params.get('range') === 'all' ? 'all' : params.get('range') === '7d' ? '7d' : params.get('range') === '30d' ? '30d' : 'today';
  const origin = (['owner', 'guest', 'agent', 'automation', 'unknown'].includes(params.get('origin') ?? '') ? params.get('origin') : '') as TurnOrigin | '';
  const targets = index.devices; // Colour inventory spans the full account, including a filtered-out device.
  const completion = index.threads.filter(thread => !selected || thread.deviceId === selected).map(t => t.lastTurnCompletedAt ?? '').sort().at(-1) ?? '';
  const stats = useDeviceStats(index.accountId, targets, range, completion, id => store.refreshActivity(id, true), origin);
  const { metrics, timeZone } = stats;
  const nowMs = useVisibleClock(true, 60_000);
  const now = useMemo(() => new Date(nowMs), [nowMs]);
  const [mode, setMode] = useState<'tokens' | 'cost'>('tokens');
  const pendingProtection = usePendingProtection();
  // A removed device leaves the cards at once, and its own page with it.
  const removed = (id: string) => { store.forgetDevice(id); if (selected === id) navigate('/desktop/devices', { replace: true }); };
  // Closing a device's window reads the cards' isolation again.
  const [isolationRevision, setIsolationRevision] = useState(0);
  const [nameSaved, setNameSaved] = useState(false);
  // Rename, share, isolation and removal open here, over this page (issue #114).
  const management = useDeviceActionHost({ onRemoved: removed, onRenamed: () => setNameSaved(true), onClosed: () => { setIsolationRevision(value => value + 1); void store.refresh(); } });
  const devices = useMemo(() => [...index.devices].sort((a, b) => Number(b.connected) - Number(a.connected) || a.name.localeCompare(b.name)), [index.devices]);
  const scoped = useMemo(() => devices.filter(d => !selected || d.id === selected), [devices, selected]);
  const current = devices.find(d => d.id === selected);
  const isolated = useIsolated(current, isolationRevision);
  const tab: DevicePageTab = params.get('tab') === 'settings' ? 'settings' : params.get('tab') === 'access' || params.get('tab') === 'shared' ? 'access' : 'overview';
  const saved = (id: string, r: StatsRange) => {
    const value = metrics[id]?.summaries[r];
    return r === 'today' && value?.daily[0]?.date !== dateKey(now, timeZone) ? undefined : value;
  };
  const summary = (id: string, r: StatsRange) => saved(id, r) ?? partialSummary(index, id, r, timeZone, now);
  const sources: ChartSource[] = scoped.map(d => ({ deviceId: d.id, name: d.name, summary: summary(d.id, range) }));
  const total = aggregate(sources);
  const hourly = range === 'today';
  const hasHourly = sources.some(source => supportsHourly(source.summary));
  const hourlyExcluded = scoped.filter(device => {
    const value = saved(device.id, 'today');
    // A known empty range is not excluded. Missing/offline data is not evidence
    // of an old runtime; only a successful legacy response or capability probe is.
    return !supportsHourly(value) && (value ? value.totals.turns > 0 : Boolean(metrics[device.id]?.partial && (summary(device.id, 'today').totals.turns > 0 || runtimeHours(index.activity[device.id], timeZone, now).some(value => (value ?? 0) > 0))));
  }).map(device => device.name);
  const hourlySources = sources.flatMap(source => hasHourly
    ? supportsHourly(source.summary) ? [{ deviceId: source.deviceId, name: source.name, values: Array.from({ length: 24 }, (_, hour) => source.summary.hourly!.find(row => row.hour === hour)!.turns) }] : []
    : [{ deviceId: source.deviceId, name: source.name, values: runtimeHours(index.activity[source.deviceId], timeZone, now) }]);
  const usage = hourly ? aggregateHourly(sources).map(row => ({ ...row, date: `${String(row.hour).padStart(2, '0')}:00–${String(row.hour + 1).padStart(2, '0')}:00`, tokens: row.tokens, costUsd: row.costUsd })) : total.daily;
  const partial = scoped.some(d => metrics[d.id]?.partial || !saved(d.id, range));
  const cached = scoped.filter(d => !d.connected || metrics[d.id]?.unavailable || (metrics[d.id]?.partial && saved(d.id, range)));
  const tokenUnknown = scoped.some(d => !saved(d.id, range));
  const usageSources = scoped.map(device => ({ deviceId: device.id, name: device.name, summary: origin ? (() => { const value = metrics[device.id]?.originSummaries?.[origin]?.[range]; return range === 'today' && value?.daily[0]?.date !== dateKey(now, timeZone) ? undefined : value; })() : saved(device.id, range), activity: index.activity[device.id] }));
  // An older link may still name a window with ?action=; it opens here, over the
  // device page, and the parameter goes. Settings (and "update") are a tab now.
  const linkedAction = params.get('action');
  const { manage } = management;
  useEffect(() => {
    if (!current || !linkedAction) return;
    if (linkedAction === 'settings' || linkedAction === 'update') {
      setParams(value => { const next = new URLSearchParams(value); next.delete('action'); next.set('tab', 'settings'); if (linkedAction === 'update') next.set('section', 'device'); return next; }, { replace: true });
      return;
    }
    if (!['rename', 'share', 'isolate', 'remove'].includes(linkedAction)) return;
    manage(current, linkedAction as DeviceManageAction);
    setParams(value => { const next = new URLSearchParams(value); next.delete('action'); return next; }, { replace: true });
  }, [current, linkedAction, manage, setParams]);
  const refresh = () => { void store.refresh(); stats.refresh(); };
  const reveal = (id: string) => onRevealDevice ? onRevealDevice(id) : navigate(`/?revealDevice=${encodeURIComponent(id)}`);
  const statsQuery = `?range=${range}${origin ? `&origin=${origin}` : ''}`;
  const rangeControls = <div className="pd-segment" role="group" aria-label={t('desktopDevices.timeRange')}>{(['today', '7d', '30d', 'all'] as const).map(r => <button key={r} aria-pressed={range === r} onClick={() => setParams(value => { const next = new URLSearchParams(value); next.set('range', r); return next; })}>{r === 'today' ? t('desktopDevices.today') : r === '7d' ? t('desktopDevices.sevenDays') : r === '30d' ? t('desktopDevices.thirtyDays') : t('desktopDevices.allTime')}</button>)}</div>;
  // Which devices the statistics count: every device, or one (its own page's Overview).
  const scopeControl = <Select className="pd-select" aria-label={t('desktopDevices.statsScope')} value={selected} options={[{ value: '', label: t('desktopDevices.allDevices') }, ...devices.map(device => ({ value: device.id, label: device.name }))]} onValueChange={value => navigate(value ? `/desktop/devices/${encodeURIComponent(value)}${statsQuery}` : `/desktop/devices${statsQuery}`)} />;
  const statusInfo = partial || cached.length ? <Info label={t('desktopDevices.partialRecent')} message={`${partial ? t('desktopDevices.partialNote') : ''} ${cached.map(d => t('desktopDevices.deviceCachedAge', { device: d.name, age: age(saved(d.id, range)?.generatedAt ?? d.lastHeartbeatAt, +now) })).join(t('desktopDevices.listSeparator'))}`} /> : null;
  const refreshButton = <button className="pd-icon-button" disabled={index.refreshing} onClick={refresh} aria-label={t('desktopDevices.refresh')} title={t('desktopDevices.refresh')}><RefreshCw size={14} data-icon="refresh" className={index.refreshing ? 'pd-spin' : ''} /></button>;
  const card = (d: DesktopDevice, bare = false) => {
    const metric = metrics[d.id];
    const cacheLabel = !d.connected || metric?.unavailable || (metric?.partial && saved(d.id, 'today')) ? t('desktopDevices.cachedAge', { age: age(saved(d.id, 'today')?.generatedAt ?? metric?.checkedAt ?? d.lastHeartbeatAt, +now) }) : '';
    return <DeviceCard key={d.id} bare={bare} device={d} metric={metric} today={summary(d.id, 'today')} history={(metric?.summaries['30d'] ?? partialSummary(index, d.id, '30d', timeZone, now)).daily.slice(-14)} counts={deviceCounts(index, d.id)} cacheLabel={cacheLabel} partial={!saved(d.id, 'today')} loading={Boolean(d.connected && !metric?.summaries.today && !metric?.partial && !metric?.unavailable)} skeleton={stats.skeleton} onUpdate={() => navigate(deviceSettingsHref(d.id, 'device'))} menuItems={deviceActions(d, { navigate, manage: action => management.manage(d, action), renameSupported: management.renameSupported, reveal: () => reveal(d.id) })} isolationRevision={isolationRevision} protectionPending={pendingProtection.has(d.id)} />;
  };
  // The statistics' own controls: in their own row, except on a computer's overview,
  // where they sit in the page header as before (no row pushes the statistics down).
  const statsControlsInHeader = !selected && !narrow;
  const statistics = scoped.length > 0 ? <>
    {statsControlsInHeader ? null : <div className="pd-stats-toolbar">{scopeControl}{rangeControls}</div>}
    <UsagePanel sources={usageSources} index={index} range={range} metric={mode} onMetric={setMode} origin={origin} onShowAll={() => setParams(value => { const next = new URLSearchParams(value); next.set('range', 'all'); return next; }, { replace: true })} filters={<div className="pd-origin-filter"><Select className="pd-select" aria-label={t('desktopDevices.origin')} value={origin} options={[{ value: '', label: t('desktopDevices.allOrigins') }, ...(['owner', 'guest', 'agent', 'automation', 'unknown'] as const).map(value => ({ value, label: t(`desktopDevices.${value}Origin`) }))]} onValueChange={value => setParams(previous => { const next = new URLSearchParams(previous); if (value) next.set('origin', value); else next.delete('origin'); return next; })} />{!usageSources.every(source => source.summary?.byOrigin) ? <Info message={t('desktopDevices.originUnavailable')} /> : null}</div>} />
    <section className="pd-statistics" aria-label={t('desktopDevices.statistics')}><header className="pd-usage-header"><h2>{t('desktopDevices.statistics')}</h2><Info message={`${t('desktopDevices.dateBasis', { timeZone })} ${t('desktopDevices.footnote')}`} /><div className="pd-stat-summary"><span title={t('desktopDevices.successDefinition')} aria-label={t('desktopDevices.successRate')}><CheckCircle2 size={12} aria-hidden />{total.successRate == null ? '—' : `${(total.successRate * 100).toFixed(1)}%`}</span><span title={t('desktopDevices.durationDefinition')} aria-label={t('desktopDevices.averageDuration')}><Timer size={12} aria-hidden />{duration(total.avgDurationMs)}</span></div></header>
    <div className="pd-charts-grid"><section className="pd-chart-panel"><header><h3>{hourly ? hasHourly ? t('desktopDevices.hourlyTurns') : t('desktopDevices.hourlyRuntime') : t('desktopDevices.dailyTurns')}</h3></header>{hourly ? <HourlyActivityChart excluded={hasHourly ? hourlyExcluded : []} sources={hourlySources} runtime={!hasHourly} /> : <TurnsChart sources={sources} dates={range === 'all' ? total.daily.map(day => day.date) : calendarDays(range, timeZone, now)} />}</section><section className="pd-chart-panel"><header><h3>{t(hourly ? mode === 'tokens' ? 'desktopDevices.hourlyTokens' : 'desktopDevices.hourlyCost' : mode === 'tokens' ? 'desktopDevices.dailyTokens' : 'desktopDevices.dailyCost')}</h3></header>{!hourly && tokenUnknown && !total.tokens ? <div className="pd-chart-empty">{t('desktopDevices.noRangeUsage')}</div> : <UsageChart excluded={hourly ? hourlyExcluded : []} daily={usage} mode={mode} hourly={hourly} />}</section></div>
    </section>
  </> : null;
  const colours = (children: ReactNode) => <UsageColors deviceIds={index.devices.map(device => device.id)} metrics={metrics}>{children}</UsageColors>;

  if (selected) {
    if (!current) return <div className="pd-page" data-page="device" data-testid="desktop-devices">
      <nav className="pd-breadcrumb"><Link to="/desktop/devices"><ArrowLeft size={14} aria-hidden />{t('desktopDevices.allDevices')}</Link></nav>
      {index.hydrated && !index.refreshing ? <div className="pd-empty"><Monitor size={28} /><h2>{t('desktopDevices.deviceMissing')}</h2><Link to="/desktop/devices">{t('desktopDevices.backToDevices')}</Link></div> : <div className="product-skeleton h-24 w-full" role="status" aria-busy="true" />}
    </div>;
    const portalDevice = portal?.devices.find(device => device.id === current.id);
    const chooseTab = (next: DevicePageTab) => navigate(next === 'overview' && params.get('range') ? devicePageHref(current.id) + statsQuery : devicePageHref(current.id, next));
    return colours(<div className="pd-page pd-device-page" data-page="device" data-testid="desktop-devices">
      <nav className="pd-breadcrumb"><Link to="/desktop/devices"><ArrowLeft size={14} aria-hidden />{t('desktopDevices.allDevices')}</Link></nav>
      <header className="pd-device-page-header">
        <h1 title={current.name}>{current.name}</h1>
        {nameSaved ? <span className="pd-saved" role="status">{t('desktopDevices.nameSaved')}</span> : null}
        <div className="pd-header-actions">
          {/* Opening is never refused: an offline device shows its workspaces page, which says it is offline. */}
          {tab === 'overview' ? <>{statusInfo}{refreshButton}</> : null}
          <Link className="relay-button-primary pd-open-device" to={workspacesHref(current.id)}><ArrowRight size={14} aria-hidden />{t('devices.manageOpen')}</Link>
          {/* The device's one menu on its own page (§3); its page is "Device details" already. */}
          <ManagedObjectMenu name={current.name} items={deviceActions(current, { navigate, manage: action => management.manage(current, action), renameSupported: management.renameSupported, reveal: () => reveal(current.id), details: false })} className="pd-icon-button pd-device-menu" />
        </div>
      </header>
      {portalDevice ? <DeviceStatusLine device={portalDevice} portal={portal} isolated={isolated} /> : <div className="device-status-line"><DeviceStateMark state={current.connected ? 'online' : 'offline'} /></div>}
      <div className="devices-page-tabs pd-device-tabs" role="tablist" aria-label={t('desktopDevices.devicePage')}>{DEVICE_TABS.map(id => <button key={id} type="button" role="tab" id={`device-page-tab-${id}`} aria-selected={tab === id} aria-controls="device-page-panel" tabIndex={tab === id ? 0 : -1} onClick={() => chooseTab(id)} onKeyDown={event => {
        if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
        event.preventDefault();
        const at = DEVICE_TABS.indexOf(tab);
        const next = event.key === 'Home' ? DEVICE_TABS[0]! : event.key === 'End' ? DEVICE_TABS.at(-1)! : DEVICE_TABS[(at + (event.key === 'ArrowRight' ? 1 : DEVICE_TABS.length - 1)) % DEVICE_TABS.length]!;
        chooseTab(next); document.getElementById(`device-page-tab-${next}`)?.focus();
      }}>{t(id === 'overview' ? 'desktopDevices.tabOverview' : id === 'settings' ? 'desktopDevices.tabSettings' : 'desktopDevices.access')}</button>)}</div>
      <div role="tabpanel" id="device-page-panel" aria-labelledby={`device-page-tab-${tab}`} className="pd-device-panel">
        {tab === 'overview' ? <>
          <section className="pd-device-grid is-detail" aria-label={t('desktopDevices.overview')}>{card(current, true)}</section>
          {statistics}
        </> : tab === 'settings' ? <DeviceSettingsTab deviceId={current.id} onRenamed={() => { setNameSaved(true); void store.refresh(); }} />
        : <DeviceManagement key={current.id} pane="access" deviceId={current.id} />}
      </div>
      {management.windows}
    </div>);
  }

  const tabs = <DevicesPageTabs devices={devices.length} shared={sharedItems(portal, 'in').length + sharedItems(portal, 'out').length} pending={attention.size} />;
  if (params.get('tab') === 'shared') return <div className="pd-page" data-testid="desktop-devices">{tabs}<DeviceManagement sharingOnly /></div>;
  return colours(<div className="pd-page" data-page="devices" data-testid="desktop-devices">
    {tabs}
    <div role="tabpanel" id="devices-page-panel-devices" aria-labelledby="devices-page-tab-devices">
    <header className="pd-page-header"><h1>{t('desktopDevices.devices')}</h1><span className="pd-device-count" title={t('desktopDevices.deviceCount', { online: devices.filter(d => d.connected).length, total: devices.length })}>{devices.filter(d => d.connected).length}/{devices.length}</span>{statsControlsInHeader && scoped.length ? <>{scopeControl}{rangeControls}</> : null}<div className="pd-header-actions"><UnprotectedDevicesButton devices={portal?.devices ?? []} /><button aria-controls="add-device-form" className="relay-button-primary pd-add-device" onClick={management.add}>{t('devices.addDevice')}</button>{nameSaved ? <span role="status">{t('desktopDevices.nameSaved')}</span> : null}{statusInfo}{refreshButton}</div></header>
    <section className="pd-device-grid" aria-label={t('desktopDevices.overview')}>{devices.map(d => card(d))}{!devices.length && index.hydrated && !index.refreshing ? <div className="pd-empty"><Monitor size={28} /><h2>{t('desktopDevices.noDevices')}</h2><p>{t('desktopDevices.noDevicesHint')}</p>{/* An empty list offers its first step, as the workspace list does. */}<button className="relay-button-primary pd-add-device" onClick={management.add}>{t('devices.addDevice')}</button></div> : null}</section>
    {/* A phone shows its devices first; the account's statistics fold away below them (issue #115). */}
    {statistics ? narrow ? <details className="pd-stats-fold"><summary className="settings-summary">{t('desktopDevices.statistics')}</summary>{statistics}</details> : statistics : null}
    </div>
    <DeviceManagement sharingOnly dialogsOnly />
    {management.windows}
  </div>);
}

/** Whether the device isolates itself, read from it while it is online. */
function useIsolated(device: DesktopDevice | undefined, revision: number) {
  const [isolated, setIsolated] = useState<{ id: string; value: boolean } | null>(null);
  const id = device?.id, connected = device?.connected;
  useEffect(() => {
    if (!id || !connected) return;
    let alive = true;
    fetchAgentIsolation(id).then(value => { if (alive) setIsolated({ id, value: Boolean(value.device) }); }).catch(() => {});
    return () => { alive = false; };
  }, [id, connected, revision]);
  return Boolean(id && isolated?.id === id && isolated.value);
}

/**
 * Everything about one device, in the settings window's own layout: its name,
 * connection and isolation, then its Supervisor, Harnesses, upstreams, resources,
 * isolated environments and workspace defaults, all acting on this device. The
 * section is in the address (`?section=`), so a link opens it.
 */
function DeviceSettingsTab({ deviceId, onRenamed }: { deviceId: string; onRenamed: () => void }) {
  useI18n();
  const [params, setParams] = useSearchParams();
  const sections = deviceSettingsSections(deviceId, <DeviceManagement key={deviceId} pane="settings" deviceId={deviceId} onRenamed={onRenamed} />);
  const requested = params.get('section');
  const section = sections.some(entry => entry.id === requested) ? requested! : 'general';
  return <div className="pd-device-settings">
    <SettingsPanels sections={sections} selected={section} onSelect={id => setParams(value => { const next = new URLSearchParams(value); next.set('tab', 'settings'); if (id === 'general') next.delete('section'); else next.set('section', id); return next; }, { replace: true })} />
  </div>;
}
