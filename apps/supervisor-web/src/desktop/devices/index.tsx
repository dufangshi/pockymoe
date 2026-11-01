import { useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { ChevronLeft, Monitor, RefreshCw } from 'lucide-react';
import { useI18n, type TranslationKey } from '@pockymoe/thread-ui/i18n';
import { ConfirmDialog, ErrorMark } from '@pockymoe/thread-ui';
import { ApiError, request } from '../../lib/api';
import { appUrl } from '../../lib/webBase';
import { FormDialog } from '../../components/FormDialog';
import { RenameDialog } from '../../components/RenameDialog';
import { RuntimeManagement } from '../../components/RuntimeManagement';
import { useDesktopSelector, useDesktopStore } from '../data';
import { desktopHost } from '../host';
import { DeviceCard } from './DeviceCard';
import { HourlyActivityChart, TurnsChart, UsageChart, type ChartSource } from './Charts';
import { age, aggregate, aggregateHourly, calendarDays, compact, dateKey, deviceColor, deviceCounts, duration, exact, lifetimeActivity, money, partialSummary, runtimeHours, supportsHourly, turns, type StatsRange } from './model';
import { useDeviceStats } from './useDeviceStats';
import './devices.css';

type DeviceError = { key: TranslationKey } | { message: string };
export interface DevicesPageProps { onRevealDevice?(deviceId: string): void; onNewThread?(deviceId: string): void }
export default function DevicesPage({ onRevealDevice, onNewThread }: DevicesPageProps) {
  const { t } = useI18n();
  const index = useDesktopSelector(index => index);
  const store = useDesktopStore();
  const navigate = useNavigate();
  const route = useParams<{ id: string; deviceId: string }>();
  const [params, setParams] = useSearchParams();
  const selected = route.id ?? route.deviceId ?? params.get('device') ?? '';
  const range: StatsRange = params.get('range') === '7d' ? '7d' : params.get('range') === '30d' ? '30d' : 'today';
  const targets = selected ? index.devices.filter(device => device.id === selected) : index.devices;
  const completion = index.threads.filter(thread => !selected || thread.deviceId === selected).map(t => t.lastTurnCompletedAt ?? '').sort().at(-1) ?? '';
  const stats = useDeviceStats(index.accountId, targets, range, completion, id => store.refreshActivity(id, true));
  const { metrics, timeZone } = stats;
  const [now, setNow] = useState(() => new Date());
  const [mode, setMode] = useState<'tokens' | 'cost'>('tokens');
  const [renaming, setRenaming] = useState<string | null>(null);
  const [discarding, setDiscarding] = useState(false);
  const [updating, setUpdating] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DeviceError | null>(null);
  const errorText = error ? 'key' in error ? t(error.key) : error.message : null;
  const [renameSupported, setRenameSupported] = useState(true);
  const [nameSaved, setNameSaved] = useState(false);
  const devices = useMemo(() => [...index.devices].sort((a, b) => Number(b.connected) - Number(a.connected) || a.name.localeCompare(b.name)), [index.devices]);
  const scoped = useMemo(() => devices.filter(d => !selected || d.id === selected), [devices, selected]);
  const current = devices.find(d => d.id === selected);
  const saved = (id: string, r: StatsRange) => {
    const value = metrics[id]?.summaries[r];
    return r === 'today' && value?.daily[0]?.date !== dateKey(now, timeZone) ? undefined : value;
  };
  const summary = (id: string, r: StatsRange) => saved(id, r) ?? partialSummary(index, id, r, timeZone, now);
  const sources: ChartSource[] = scoped.map(d => ({ deviceId: d.id, name: d.name, summary: summary(d.id, range) }));
  const total = aggregate(sources);
  const hourly = range === 'today';
  const hourlyComplete = sources.length > 0 && sources.every(source => supportsHourly(source.summary));
  const hourlySources = useMemo(() => scoped.map(device => ({ deviceId: device.id, name: device.name, values: hourlyComplete
    ? Array.from({ length: 24 }, (_, hour) => metrics[device.id]!.summaries.today!.hourly!.find(row => row.hour === hour)!.turns)
    : runtimeHours(index.activity[device.id], timeZone, now) })), [hourlyComplete, index.activity, metrics, now, scoped, timeZone]);
  const usage = hourly ? aggregateHourly(sources).map(row => ({ date: `${String(row.hour).padStart(2, '0')}:00–${String(row.hour + 1).padStart(2, '0')}:00`, tokens: row.tokens, costUsd: row.costUsd })) : total.daily;
  const partial = scoped.some(d => metrics[d.id]?.partial || !saved(d.id, range));
  const cached = scoped.filter(d => !d.connected || metrics[d.id]?.unavailable || (metrics[d.id]?.partial && saved(d.id, range)));
  const tokenUnknown = scoped.some(d => !saved(d.id, range));
  const lifetime = scoped.filter(d => metrics[d.id]?.partial && index.activity[d.id]).map(d => ({ device: d, ...lifetimeActivity(index, d.id, index.activity[d.id]!) }));
  useEffect(() => { const timer = setInterval(() => setNow(new Date()), 60_000); return () => clearInterval(timer); }, []);
  useEffect(() => {
    const action = params.get('action');
    if (!current || !action) return;
    if (action === 'rename' && renameSupported) { setName(current.name); setRenaming(current.id); }
    if (action === 'update' && current.connected) setUpdating(current.id);
    setParams(value => { const next = new URLSearchParams(value); next.delete('action'); return next; }, { replace: true });
  }, [current, params, renameSupported, setParams]);
  const refresh = () => { void store.refresh(); stats.refresh(); };
  const reveal = (id: string) => onRevealDevice ? onRevealDevice(id) : navigate(`/?revealDevice=${encodeURIComponent(id)}`);
  const newThread = (id: string) => onNewThread ? onNewThread(id) : navigate(`/devices/${encodeURIComponent(id)}/threads/new`);
  async function rename() {
    if (!renaming || !name.trim() || busy) return;
    const hasControl = [...name].some(character => { const code = character.codePointAt(0)!; return code < 32 || (code >= 127 && code <= 159); });
    if ([...name.trim()].length > 64 || hasControl) { setError({ key: 'desktopDevices.invalidName' }); return; }
    setBusy(true); setError(null);
    try {
      await request(`relay/devices/${encodeURIComponent(renaming)}`, { method: 'PATCH', body: JSON.stringify({ name: name.trim() }) });
      await store.refresh(); setRenaming(null); setNameSaved(true);
    } catch (cause) {
      if (cause instanceof ApiError && [404, 405].includes(cause.statusCode)) { setRenameSupported(false); setRenaming(null); setError({ key: 'desktopDevices.renameUnsupported' }); }
      else setError(cause instanceof Error ? { message: cause.message } : { key: 'desktopDevices.renameFailed' });
    } finally { setBusy(false); }
  }
  function cancelRename() {
    if (busy) return;
    const original = devices.find(device => device.id === renaming)?.name ?? '';
    if (name !== original) { setDiscarding(true); return; }
    setError(null); setRenaming(null);
  }
  const rangeControls = <div className="pd-segment" role="group" aria-label={t('desktopDevices.timeRange')}>{(['today', '7d', '30d'] as const).map(r => <button key={r} aria-pressed={range === r} onClick={() => setParams(value => { const next = new URLSearchParams(value); next.set('range', r); return next; })}>{r === 'today' ? t('desktopDevices.today') : r === '7d' ? t('desktopDevices.sevenDays') : t('desktopDevices.thirtyDays')}</button>)}</div>;
  return <div className="pd-page" data-page="devices" data-testid="desktop-devices">
    <header className="pd-page-header"><div>{selected ? <Link to="/desktop/devices" className="pd-back"><ChevronLeft size={13} />{t('desktopDevices.allDevices')}</Link> : <p className="pd-eyebrow">{t('desktopDevices.intro')}</p>}<h1>{current?.name ?? (selected ? t('desktopDevices.deviceDetails') : t('desktopDevices.devices'))}</h1><p>{selected ? t('desktopDevices.detailIntro') : t('desktopDevices.deviceCount', { online: devices.filter(d => d.connected).length, total: devices.length })}</p></div><div className="pd-header-actions">{nameSaved ? <span role="status">{t('desktopDevices.nameSaved')}</span> : null}<button className="pd-button host-secondary-button" disabled={index.refreshing} onClick={refresh}><RefreshCw size={14} data-icon="refresh" className={index.refreshing ? 'pd-spin' : ''} />{t('desktopDevices.refresh')}</button>{error && !renaming ? <ErrorMark message={errorText!} onRetry={refresh} /> : null}</div></header>
    {selected && !current && index.hydrated && !index.refreshing ? <div className="pd-empty"><Monitor size={28} /><h2>{t('desktopDevices.deviceMissing')}</h2><Link to="/desktop/devices">{t('desktopDevices.backToDevices')}</Link></div> : null}
    <section className={`pd-device-grid ${selected ? 'is-detail' : ''}`} aria-label={t('desktopDevices.overview')}>{scoped.map(d => {
      const metric = metrics[d.id];
      const cacheLabel = !d.connected || metric?.unavailable || (metric?.partial && saved(d.id, 'today')) ? t('desktopDevices.cachedAge', { age: age(saved(d.id, 'today')?.generatedAt ?? metric?.checkedAt ?? d.lastHeartbeatAt, +now) }) : '';
      return <DeviceCard key={d.id} device={d} metric={metric} today={summary(d.id, 'today')} history={(metric?.summaries['30d'] ?? partialSummary(index, d.id, '30d', timeZone, now)).daily.slice(-14)} counts={deviceCounts(index, d.id)} cacheLabel={cacheLabel} partial={!saved(d.id, 'today')} loading={Boolean(d.connected && !metric?.summaries.today && !metric?.partial && !metric?.unavailable)} skeleton={stats.skeleton} renameSupported={renameSupported} onReveal={() => reveal(d.id)} onNewThread={() => newThread(d.id)} onRename={() => { setError(null); setName(d.name); setRenaming(d.id); }} onUpdate={() => setUpdating(d.id)} onBrowser={() => desktopHost()?.shell.openExternal(new URL(appUrl(`/devices/${encodeURIComponent(d.id)}/workspaces`), location.origin).href)} />;
    })}{!selected && !devices.length && index.hydrated && !index.refreshing ? <div className="pd-empty"><Monitor size={28} /><h2>{t('desktopDevices.noDevices')}</h2><p>{t('desktopDevices.noDevicesHint')}</p></div> : null}</section>
    {scoped.length > 0 ? <section className="pd-statistics" aria-label={t('desktopDevices.statistics')}>
      <header className="pd-stats-header"><div><h2>{t('desktopDevices.statistics')}</h2><p>{t('desktopDevices.dateBasis', { timeZone })}</p></div><div className="pd-stats-controls"><select aria-label={t('desktopDevices.statsDevice')} value={selected} onChange={event => navigate(event.target.value ? `/desktop/devices/${encodeURIComponent(event.target.value)}?range=${range}` : `/desktop/devices?range=${range}`)}><option value="">{t('desktopDevices.allDevices')}</option>{devices.map(d => <option key={d.id} value={d.id}>{d.name}</option>)}</select>{rangeControls}</div></header>
      {(partial || cached.length > 0) ? <p className="pd-data-note">{partial ? t('desktopDevices.partialNote') : ''}{cached.length ? ` ${cached.map(d => t('desktopDevices.deviceCachedAge', { device: d.name, age: age(saved(d.id, range)?.generatedAt ?? d.lastHeartbeatAt, +now) })).join(t('desktopDevices.listSeparator'))}` : ''}</p> : null}
      <div className="pd-kpis">{[
        [t('desktopDevices.totalTurns'), compact(total.turns), t('desktopDevices.includesRunning')], [t('desktopDevices.successRate'), total.successRate == null ? '—' : `${(total.successRate * 100).toFixed(1)}%`, t('desktopDevices.successDefinition')],
        [t('desktopDevices.averageDuration'), duration(total.avgDurationMs), t('desktopDevices.durationDefinition')], [t('desktopDevices.totalTokens'), tokenUnknown && !total.tokens ? '—' : compact(total.tokens), t('desktopDevices.cachedInputHint', { value: tokenUnknown ? '—' : compact(total.cachedTokens) })], [t('desktopDevices.estimatedCost'), `≈ ${money(total.costUsd)}`, t('desktopDevices.noCostGuess')],
      ].map(([label, value, hint]) => <div className="pd-kpi" key={label}><span>{label}</span><strong title={label === t('desktopDevices.totalTokens') ? exact(total.tokens) : label === t('desktopDevices.totalTurns') ? exact(total.turns) : undefined}>{value}</strong><small>{hint}</small></div>)}</div>
      <div className="pd-charts-grid"><section className="pd-chart-panel"><header><h3>{hourly ? hourlyComplete ? t('desktopDevices.hourlyTurns') : t('desktopDevices.hourlyRuntime') : t('desktopDevices.dailyTurns')}</h3><span>{hourly && !hourlyComplete ? t('desktopDevices.activityPartial') : t('desktopDevices.byDevice')}</span></header>{hourly ? <HourlyActivityChart sources={hourlySources} runtime={!hourlyComplete} /> : <TurnsChart sources={sources} dates={calendarDays(range, timeZone, now)} />}</section><section className="pd-chart-panel"><header><h3>{t(hourly ? mode === 'tokens' ? 'desktopDevices.hourlyTokens' : 'desktopDevices.hourlyCost' : mode === 'tokens' ? 'desktopDevices.dailyTokens' : 'desktopDevices.dailyCost')}</h3><div className="pd-segment pd-segment-small" role="group" aria-label={t('desktopDevices.usageMetric')}><button aria-pressed={mode === 'tokens'} onClick={() => setMode('tokens')}>{t('desktopDevices.tokens')}</button><button aria-pressed={mode === 'cost'} onClick={() => setMode('cost')}>{t('desktopDevices.cost')}</button></div></header>{!hourly && tokenUnknown && !total.tokens ? <div className="pd-chart-empty">{t('desktopDevices.noRangeUsage')}</div> : <><UsageChart daily={usage} mode={mode} hourly={hourly} />{hourly && !hourlyComplete ? <p className="pd-hourly-note">{t('desktopDevices.hourlyUnavailable')}</p> : null}</>}</section></div>
      <div className="pd-breakdowns"><section className="pd-chart-panel"><header><h3>{t('desktopDevices.models')}</h3><span>{t('desktopDevices.byTurns')}</span></header>{total.models.length ? total.models.map((m, i) => <div className="pd-model-row" key={m.model} tabIndex={0} aria-label={t('desktopDevices.modelUsageLabel', { model: m.model, turns: turns(m.turns), tokens: exact(m.tokens) })}><div><span>{m.model || t('desktopDevices.unknownModel')}</span><b>{exact(m.turns)} <small>{t('desktopDevices.turnUnit', { count: m.turns })}</small></b></div><div className="pd-bar-track"><span style={{ width: `${m.turns / Math.max(1, total.turns) * 100}%`, background: deviceColor(String(i)) }} /></div></div>) : <div className="pd-chart-empty">{t('desktopDevices.noModels')}</div>}</section>
      <section className="pd-chart-panel"><header><h3>{t('desktopDevices.busyWorkspaces')}</h3><span>{t('desktopDevices.topFive')}</span></header><div className="pd-ranked-list">{total.workspaces.map((w, i) => <Link key={`${w.deviceId}:${w.workspaceId}`} to={`/devices/${encodeURIComponent(w.deviceId)}/threads?workspaceId=${encodeURIComponent(w.workspaceId)}`}><span className="pd-rank">{i + 1}</span><span className="pd-ranked-title">{w.label}<small>{devices.find(d => d.id === w.deviceId)?.name}</small></span><b>{turns(w.turns)}</b></Link>)}{!total.workspaces.length ? <div className="pd-chart-empty">{t('desktopDevices.noWorkspaces')}</div> : null}</div></section>
      <section className="pd-chart-panel"><header><h3>{t('desktopDevices.longestThreads')}</h3><span>{t('desktopDevices.topFive')}</span></header><div className="pd-ranked-list">{total.longest.map((t, i) => <Link key={`${t.deviceId}:${t.threadId}`} to={`/devices/${encodeURIComponent(t.deviceId)}/threads/${encodeURIComponent(t.threadId)}`}><span className="pd-rank">{i + 1}</span><span className="pd-ranked-title">{t.title}<small>{devices.find(d => d.id === t.deviceId)?.name}</small></span><b>{duration(t.durationMs)}</b></Link>)}{!total.longest.length ? <div className="pd-chart-empty">{t('desktopDevices.noDurations')}</div> : null}</div></section></div>
      {lifetime.length ? <section className="pd-lifetime pd-chart-panel" aria-label={t('desktopDevices.lifetimeFallback')}><header><h3>{t('desktopDevices.lifetimeUsage')}</h3><span>{t('desktopDevices.lifetimeScope')}</span></header>{lifetime.map(row => <div key={row.device.id}><strong>{row.device.name}</strong><span>{t('desktopDevices.lifetimeLine', { turns: turns(row.turns), tokens: compact(row.tokens), cost: money(row.costUsd) })}</span><small>{row.workspaces.map(w => `${w.label} ${turns(w.turns)}`).join(' · ')}</small></div>)}</section> : null}
      <p className="pd-footnote">{t('desktopDevices.footnote')}</p>
    </section> : null}
    <RenameDialog open={Boolean(renaming)} title={t('desktopDevices.renameDevice')} label={t('desktopDevices.deviceName')} value={name} busy={busy} error={errorText} onChange={value => { setName(value); setError(null); }} onCancel={() => { void cancelRename(); }} onSubmit={rename} />
    <ConfirmDialog open={discarding} title={t('desktopDevices.discardTitle')} description={t('desktopDevices.discardDescription')} confirmLabel={t('desktopDevices.discard')} tone="primary" onCancel={() => setDiscarding(false)} onConfirm={() => { setDiscarding(false); setError(null); setRenaming(null); }} />
    {updating ? <FormDialog title={`${devices.find(d => d.id === updating)?.name ?? t('desktopDevices.device')} · Pockymoe`} description={t('desktopDevices.runtimeDescription')} onClose={() => setUpdating(null)}><RuntimeManagement deviceId={updating} view="supervisor" /></FormDialog> : null}
  </div>;
}
