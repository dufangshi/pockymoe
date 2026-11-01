import { useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { ChevronLeft, Monitor, RefreshCw } from 'lucide-react';
import { confirmAsync, ErrorMark } from '@pockymoe/thread-ui';
import { ApiError, request } from '../../lib/api';
import { appUrl } from '../../lib/webBase';
import { FormDialog } from '../../components/FormDialog';
import { RenameDialog } from '../../components/RenameDialog';
import { RuntimeManagement } from '../../components/RuntimeManagement';
import { useDesktopSelector, useDesktopStore } from '../data';
import { desktopHost } from '../host';
import { DeviceCard } from './DeviceCard';
import { HourlyActivityChart, TurnsChart, UsageChart, type ChartSource } from './Charts';
import { age, aggregate, aggregateHourly, calendarDays, compact, dateKey, deviceColor, deviceCounts, duration, exact, lifetimeActivity, money, partialSummary, runtimeHours, supportsHourly, type StatsRange } from './model';
import { useDeviceStats } from './useDeviceStats';
import './devices.css';

export interface DevicesPageProps { onRevealDevice?(deviceId: string): void; onNewThread?(deviceId: string): void }
export default function DevicesPage({ onRevealDevice, onNewThread }: DevicesPageProps) {
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
  const [updating, setUpdating] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [renameSupported, setRenameSupported] = useState(true);
  const [notice, setNotice] = useState('');
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
    if ([...name.trim()].length > 64 || hasControl) { setError('设备名称须为 1–64 个字符，不能包含控制字符。'); return; }
    setBusy(true); setError(null);
    try {
      await request(`relay/devices/${encodeURIComponent(renaming)}`, { method: 'PATCH', body: JSON.stringify({ name: name.trim() }) });
      await store.refresh(); setRenaming(null); setNotice('设备名称已保存');
    } catch (cause) {
      if (cause instanceof ApiError && [404, 405].includes(cause.statusCode)) { setRenameSupported(false); setRenaming(null); setError('当前 Relay 暂不支持重命名设备。'); }
      else setError(cause instanceof Error ? cause.message : '无法保存设备名称。');
    } finally { setBusy(false); }
  }
  async function cancelRename() {
    if (busy) return;
    const original = devices.find(device => device.id === renaming)?.name ?? '';
    if (name !== original && !(await confirmAsync({ title: '放弃名称修改？', description: '尚未保存的设备名称将被丢弃。', confirmLabel: '放弃修改', tone: 'primary' }))) return;
    setError(null); setRenaming(null);
  }
  const rangeControls = <div className="pd-segment" role="group" aria-label="统计时间范围">{(['today', '7d', '30d'] as const).map(r => <button key={r} aria-pressed={range === r} onClick={() => setParams(value => { const next = new URLSearchParams(value); next.set('range', r); return next; })}>{r === 'today' ? '今天' : r === '7d' ? '7 天' : '30 天'}</button>)}</div>;
  return <div className="pd-page" data-page="devices" data-testid="desktop-devices">
    <header className="pd-page-header"><div>{selected ? <Link to="/desktop/devices" className="pd-back"><ChevronLeft size={13} />全部设备</Link> : <p className="pd-eyebrow">所有设备，一处掌握</p>}<h1>{current?.name ?? (selected ? '设备详情' : '设备')}</h1><p>{selected ? '运行环境与使用统计' : `${devices.filter(d => d.connected).length} 台在线 · ${devices.length} 台设备`}</p></div><div className="pd-header-actions">{notice ? <span role="status">{notice}</span> : null}<button className="pd-button host-secondary-button" disabled={index.refreshing} onClick={refresh}><RefreshCw size={14} data-icon="refresh" className={index.refreshing ? 'pd-spin' : ''} />刷新</button>{error && !renaming ? <ErrorMark message={error} onRetry={refresh} /> : null}</div></header>
    {selected && !current && index.hydrated && !index.refreshing ? <div className="pd-empty"><Monitor size={28} /><h2>找不到这台设备</h2><Link to="/desktop/devices">返回全部设备</Link></div> : null}
    <section className={`pd-device-grid ${selected ? 'is-detail' : ''}`} aria-label="设备总览">{scoped.map(d => {
      const metric = metrics[d.id];
      const cacheLabel = !d.connected || metric?.unavailable || (metric?.partial && saved(d.id, 'today')) ? `缓存 · ${age(saved(d.id, 'today')?.generatedAt ?? metric?.checkedAt ?? d.lastHeartbeatAt, +now)}` : '';
      return <DeviceCard key={d.id} device={d} metric={metric} today={summary(d.id, 'today')} history={(metric?.summaries['30d'] ?? partialSummary(index, d.id, '30d', timeZone, now)).daily.slice(-14)} counts={deviceCounts(index, d.id)} cacheLabel={cacheLabel} partial={!saved(d.id, 'today')} loading={Boolean(d.connected && !metric?.summaries.today && !metric?.partial && !metric?.unavailable)} skeleton={stats.skeleton} renameSupported={renameSupported} onReveal={() => reveal(d.id)} onNewThread={() => newThread(d.id)} onRename={() => { setError(null); setName(d.name); setRenaming(d.id); }} onUpdate={() => setUpdating(d.id)} onBrowser={() => desktopHost()?.shell.openExternal(new URL(appUrl(`/devices/${encodeURIComponent(d.id)}/workspaces`), location.origin).href)} />;
    })}{!selected && !devices.length && index.hydrated && !index.refreshing ? <div className="pd-empty"><Monitor size={28} /><h2>还没有设备</h2><p>连接设备后，在这里查看运行状态和使用统计。</p></div> : null}</section>
    {scoped.length > 0 ? <section className="pd-statistics" aria-label="使用统计">
      <header className="pd-stats-header"><div><h2>使用统计</h2><p>{timeZone} · 按轮次开始日期</p></div><div className="pd-stats-controls"><select aria-label="统计设备" value={selected} onChange={event => navigate(event.target.value ? `/desktop/devices/${encodeURIComponent(event.target.value)}?range=${range}` : `/desktop/devices?range=${range}`)}><option value="">全部设备</option>{devices.map(d => <option key={d.id} value={d.id}>{d.name}</option>)}</select>{rangeControls}</div></header>
      {(partial || cached.length > 0) ? <p className="pd-data-note">{partial ? '部分数据 · 不支持日历统计的设备保留缓存；没有缓存时，仅计入索引中可见的最近轮次。' : ''}{cached.length ? ` ${cached.map(d => `${d.name}：缓存 · ${age(saved(d.id, range)?.generatedAt ?? d.lastHeartbeatAt, +now)}`).join('；')}` : ''}</p> : null}
      <div className="pd-kpis">{[
        ['总轮数', compact(total.turns), '包含进行中的轮次'], ['成功率', total.successRate == null ? '—' : `${(total.successRate * 100).toFixed(1)}%`, '完成 / 全部终态轮次'],
        ['平均轮时长', duration(total.avgDurationMs), '包含工具与等待时间'], ['总 tokens', tokenUnknown && !total.tokens ? '—' : compact(total.tokens), `缓存读取 ${tokenUnknown ? '—' : compact(total.cachedTokens)} · 已计入输入`], ['估算费用', `≈ ${money(total.costUsd)}`, '未知费用不推算'],
      ].map(([label, value, hint]) => <div className="pd-kpi" key={label}><span>{label}</span><strong title={label === '总 tokens' ? exact(total.tokens) : label === '总轮数' ? exact(total.turns) : undefined}>{value}</strong><small>{hint}</small></div>)}</div>
      <div className="pd-charts-grid"><section className="pd-chart-panel"><header><h3>{hourly ? hourlyComplete ? '今日每小时轮数' : '今日每小时运行时长' : '每日轮数'}</h3><span>{hourly && !hourlyComplete ? '活动采样 · 部分数据' : '按设备'}</span></header>{hourly ? <HourlyActivityChart sources={hourlySources} runtime={!hourlyComplete} /> : <TurnsChart sources={sources} dates={calendarDays(range, timeZone, now)} />}</section><section className="pd-chart-panel"><header><h3>{hourly ? '每小时' : '每日'}{mode === 'tokens' ? ' Token 用量' : '估算费用'}</h3><div className="pd-segment pd-segment-small" role="group" aria-label="用量指标"><button aria-pressed={mode === 'tokens'} onClick={() => setMode('tokens')}>Tokens</button><button aria-pressed={mode === 'cost'} onClick={() => setMode('cost')}>费用</button></div></header>{!hourly && tokenUnknown && !total.tokens ? <div className="pd-chart-empty">暂无范围内的用量数据</div> : <><UsageChart daily={usage} mode={mode} hourly={hourly} />{hourly && !hourlyComplete ? <p className="pd-hourly-note">部分设备未提供小时用量；可选择其他设备查看。</p> : null}</>}</section></div>
      <div className="pd-breakdowns"><section className="pd-chart-panel"><header><h3>模型分布</h3><span>按轮数</span></header>{total.models.length ? total.models.map((m, i) => <div className="pd-model-row" key={m.model} tabIndex={0} aria-label={`${m.model}：${exact(m.turns)} 轮，${exact(m.tokens)} tokens`}><div><span>{m.model || '未知模型'}</span><b>{exact(m.turns)} <small>轮</small></b></div><div className="pd-bar-track"><span style={{ width: `${m.turns / Math.max(1, total.turns) * 100}%`, background: deviceColor(String(i)) }} /></div></div>) : <div className="pd-chart-empty">所选范围暂无模型用量</div>}</section>
      <section className="pd-chart-panel"><header><h3>最忙的工作区</h3><span>前 5 名</span></header><div className="pd-ranked-list">{total.workspaces.map((w, i) => <Link key={`${w.deviceId}:${w.workspaceId}`} to={`/devices/${encodeURIComponent(w.deviceId)}/threads?workspaceId=${encodeURIComponent(w.workspaceId)}`}><span className="pd-rank">{i + 1}</span><span className="pd-ranked-title">{w.label}<small>{devices.find(d => d.id === w.deviceId)?.name}</small></span><b>{exact(w.turns)} 轮</b></Link>)}{!total.workspaces.length ? <div className="pd-chart-empty">所选范围暂无工作区活动</div> : null}</div></section>
      <section className="pd-chart-panel"><header><h3>平均用时最长的线程</h3><span>前 5 名</span></header><div className="pd-ranked-list">{total.longest.map((t, i) => <Link key={`${t.deviceId}:${t.threadId}`} to={`/devices/${encodeURIComponent(t.deviceId)}/threads/${encodeURIComponent(t.threadId)}`}><span className="pd-rank">{i + 1}</span><span className="pd-ranked-title">{t.title}<small>{devices.find(d => d.id === t.deviceId)?.name}</small></span><b>{duration(t.durationMs)}</b></Link>)}{!total.longest.length ? <div className="pd-chart-empty">暂无终态轮次的时长数据</div> : null}</div></section></div>
      {lifetime.length ? <section className="pd-lifetime pd-chart-panel" aria-label="累计活动回退"><header><h3>累计已记录用量</h3><span>部分数据 · 全部历史，不随上方范围变化</span></header>{lifetime.map(row => <div key={row.device.id}><strong>{row.device.name}</strong><span>{exact(row.turns)} 轮 · {compact(row.tokens)} tokens · ≈ {money(row.costUsd)}</span><small>{row.workspaces.map(w => `${w.label} ${exact(w.turns)} 轮`).join(' · ')}</small></div>)}</section> : null}
      <p className="pd-footnote">用量仅包含设备上已记录的轮次。费用为估算值，实际账单以提供方为准。</p>
    </section> : null}
    <RenameDialog open={Boolean(renaming)} title="重命名设备" label="设备名称" value={name} busy={busy} error={error} onChange={value => { setName(value); setError(null); }} onCancel={() => { void cancelRename(); }} onSubmit={rename} />
    {updating ? <FormDialog title={`${devices.find(d => d.id === updating)?.name ?? '设备'} · Pockymoe`} description="检查版本并管理这台设备的运行环境。" onClose={() => setUpdating(null)}><RuntimeManagement deviceId={updating} view="supervisor" /></FormDialog> : null}
  </div>;
}
