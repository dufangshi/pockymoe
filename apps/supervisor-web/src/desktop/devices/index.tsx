import { useEffect, useMemo, useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { ArrowUpRight, ChevronRight, Ellipsis, Monitor, Plus, RefreshCw } from 'lucide-react';
import { ApiError, request } from '../../lib/api';
import { FormDialog } from '../../components/FormDialog';
import { RuntimeManagement } from '../../components/RuntimeManagement';
import { useDesktopSelector, useDesktopStore } from '../data';
import { Sparkline, TurnsChart, UsageChart, type ChartSource } from './Charts';
import { age, aggregate, calendarDays, compact, dateKey, deviceColor, deviceCounts, duration, exact, money, newerVersion, partialSummary, type StatsRange } from './model';
import { useDeviceMetrics } from './useDeviceMetrics';
import './devices.css';

export interface DevicesPageProps {
  onRevealDevice?(deviceId: string): void;
  onNewThread?(deviceId: string): void;
}
export default function DevicesPage({ onRevealDevice, onNewThread }: DevicesPageProps) {
  const index = useDesktopSelector(index => index);
  const store = useDesktopStore();
  const navigate = useNavigate();
  const [params, setParams] = useSearchParams();
  const range: StatsRange = params.get('range') === '7d' ? '7d' : params.get('range') === '30d' ? '30d' : 'today';
  const selectedDevice = params.get('device') ?? '';
  const { metrics, showSkeleton, timeZone, refreshMetrics } = useDeviceMetrics(range);
  const [usageMode, setUsageMode] = useState<'tokens' | 'cost'>('tokens');
  const [menu, setMenu] = useState<string | null>(null);
  const [rename, setRename] = useState<string | null>(null);
  const [update, setUpdate] = useState<string | null>(null);
  const [renameUnsupported, setRenameUnsupported] = useState(false);
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [now, setNow] = useState(() => new Date());
  useEffect(() => { const timer = setInterval(() => setNow(new Date()), 60_000); return () => clearInterval(timer); }, []);
  const change = (key: string, value: string) => setParams(current => { const next = new URLSearchParams(current); if (value) next.set(key, value); else next.delete(key); return next; });
  const startRename = (id: string) => { setError(''); setName(index.devices.find(d => d.id === id)?.name ?? ''); setRename(id); setMenu(null); };
  useEffect(() => {
    const action = params.get('action');
    if (!selectedDevice || !index.devices.some(d => d.id === selectedDevice)) return;
    if (action === 'rename' && !renameUnsupported) { setName(index.devices.find(d => d.id === selectedDevice)!.name); setRename(selectedDevice); }
    if (action === 'update' && index.devices.find(d => d.id === selectedDevice)?.connected) setUpdate(selectedDevice);
    if (action) setParams(current => { const next = new URLSearchParams(current); next.delete('action'); return next; }, { replace: true });
  }, [index.devices, params, renameUnsupported, selectedDevice, setParams]);
  useEffect(() => {
    if (!menu) return;
    const close = (event: KeyboardEvent) => { if (event.key === 'Escape') { setMenu(null); document.getElementById(`rd-menu-${menu}`)?.focus(); } };
    window.addEventListener('keydown', close); return () => window.removeEventListener('keydown', close);
  }, [menu]);
  const devices = useMemo(() => [...index.devices].sort((a, b) => Number(b.connected) - Number(a.connected) || a.name.localeCompare(b.name)), [index.devices]);
  const savedSummary = (id: string, selectedRange: StatsRange) => {
    const cached = metrics[id]?.summaries[selectedRange];
    // A cached "today" from yesterday must never be labelled as today's usage.
    const validToday = selectedRange !== 'today' || cached?.daily[0]?.date === dateKey(now, timeZone);
    return cached && validToday ? cached : undefined;
  };
  const summaryFor = (id: string, selectedRange: StatsRange) => savedSummary(id, selectedRange) ?? partialSummary(index, id, selectedRange, timeZone, now);
  const scoped = devices.filter(d => !selectedDevice || d.id === selectedDevice);
  const sources: ChartSource[] = scoped.map(d => ({ deviceId: d.id, name: d.name, summary: summaryFor(d.id, range) }));
  const total = aggregate(sources);
  const partial = scoped.some(d => metrics[d.id]?.partial || !savedSummary(d.id, range));
  const cached = scoped.filter(d => !d.connected || metrics[d.id]?.unavailable);
  const tokenUnknown = scoped.some(d => !savedSummary(d.id, range));
  const hasData = sources.some(s => s.summary.totals.turns > 0);
  const openDevice = (id: string) => { if (onRevealDevice) onRevealDevice(id); else navigate(`/devices/${encodeURIComponent(id)}/workspaces`); };
  const newThread = (id: string) => { if (onNewThread) onNewThread(id); else navigate(`/devices/${encodeURIComponent(id)}/threads/new`); };
  const openBrowser = (id: string) => window.remoteCodexHost?.shell.openExternal(`${location.origin}/devices/${encodeURIComponent(id)}/workspaces`);
  async function saveName() {
    if (!rename || !name.trim() || busy) return;
    setBusy(true); setError('');
    try {
      await request(`/relay/devices/${encodeURIComponent(rename)}`, { method: 'PATCH', body: JSON.stringify({ name: name.trim() }) });
      await store.refresh(); setRename(null);
    } catch (error) {
      if (error instanceof ApiError && [404, 405].includes(error.statusCode)) { setRenameUnsupported(true); setRename(null); setNotice('当前 Relay 暂不支持重命名设备。'); }
      else setError(error instanceof Error ? error.message : '无法保存名称，请重试。');
    } finally { setBusy(false); }
  }
  return <div className="rd-page" data-testid="desktop-devices">
    <header className="rd-page-header"><div><p className="rd-eyebrow">你的工作，跨设备同步</p><h1>设备</h1><p>{devices.filter(d => d.connected).length} 台在线 <span aria-hidden="true">/</span> {devices.length} 台设备</p></div><button className="rd-button" disabled={index.refreshing} onClick={() => { void store.refresh(); refreshMetrics(); }}><RefreshCw size={14} className={index.refreshing ? 'rd-spin' : ''} />刷新</button></header>
    {notice ? <p className="rd-notice" role="status">{notice}<button aria-label="关闭提示" onClick={() => setNotice('')}>×</button></p> : null}
    {index.error ? <p className="rd-notice" role="status">设备索引暂时无法刷新 · 正在显示已保存内容</p> : null}
    {params.get('newThread') ? <p className="rd-notice">选择一台在线设备，新建线程。</p> : null}
    <section className="rd-device-grid" aria-label="设备总览">
      {devices.map(device => {
        const row = metrics[device.id]; const counts = deviceCounts(index, device.id); const today = summaryFor(device.id, 'today');
        const isPartial = row?.partial || !savedSummary(device.id, 'today');
        const isCached = !device.connected || row?.unavailable;
        const history = row?.summaries['30d']?.daily.slice(-14) ?? partialSummary(index, device.id, '30d', timeZone, now).daily.slice(-14);
        const outdated = newerVersion(row?.version, row?.latestVersion);
        return <article key={device.id} className={`rd-device-card ${!device.connected ? 'is-offline' : ''} ${selectedDevice === device.id ? 'is-selected' : ''}`} aria-label={device.name}>
          <div className="rd-card-heading"><button className="rd-device-title" title={device.name} onClick={() => change('device', device.id)}><Monitor size={15} /><strong>{device.name}</strong></button><span className="rd-presence"><i className={device.connected ? 'is-online' : ''} />{device.connected ? <>在线{row?.latencyMs != null ? <small>{row.latencyMs}ms</small> : null}</> : '离线'}</span><button id={`rd-menu-${device.id}`} className="rd-icon-button" aria-label={`${device.name} 的操作`} aria-haspopup="menu" aria-expanded={menu === device.id} onClick={() => setMenu(menu === device.id ? null : device.id)}><Ellipsis size={16} /></button></div>
          <div className="rd-version"><span>remote-codex <b>{row?.version ?? '—'}</b></span>{outdated ? <button disabled={!device.connected} onClick={() => setUpdate(device.id)} title={`有更新 ${row?.latestVersion}`}>有更新 {row?.latestVersion}<ArrowUpRight size={10} /></button> : null}</div>
          <div className="rd-counts"><span><i className="rd-running" /><b>{counts.running}</b> 运行中</span><span><i className="rd-attention" /><b>{counts.attention}</b> 需要你</span><span><b>{counts.workspaces}</b> 工作区</span><span><b>{counts.threads}</b> 线程</span></div>
          {device.connected && !row?.summaries.today && !row?.partial && !row?.unavailable ? <div className={`rd-today ${showSkeleton ? 'rd-skeleton' : ''}`} aria-label="正在加载今日统计" /> : <div className="rd-today"><span>今日</span><b>{compact(today.totals.turns)}</b> 轮 <span>·</span> {isPartial ? '—' : compact(today.totals.inputTokens + today.totals.outputTokens)} tokens <span>·</span> ≈ {money(today.totals.costUsd)}</div>}
          <div className="rd-card-bottom"><Sparkline days={history} color={deviceColor(device.id)} label={`${device.name} 近 14 天活动`} /><div>{isCached ? <span>缓存 · {age(row?.summaries.today?.generatedAt ?? row?.checkedAt ?? device.lastHeartbeatAt, +now)}</span> : isPartial ? <span>部分数据 · 最近轮次</span> : <span>近 14 天活动</span>}{!device.connected ? <small>最后在线 · {age(device.lastHeartbeatAt, +now)}</small> : null}</div></div>
          {menu === device.id ? <><button className="rd-menu-dismiss" aria-label="关闭设备操作" tabIndex={-1} onClick={() => setMenu(null)} /><div className="rd-device-menu" role="menu" aria-label={`${device.name} 的操作菜单`}><button role="menuitem" onClick={() => { setMenu(null); openDevice(device.id); }}>在树中打开<ChevronRight size={13} /></button><button role="menuitem" disabled={!device.connected} onClick={() => newThread(device.id)}>新建线程<Plus size={13} /></button>{!renameUnsupported ? <button role="menuitem" onClick={() => startRename(device.id)}>重命名</button> : null}<button role="menuitem" disabled={!device.connected} onClick={() => { setMenu(null); setUpdate(device.id); }}>检查 / 更新 runtime</button><button role="menuitem" onClick={() => openBrowser(device.id)}>在浏览器中管理<ArrowUpRight size={13} /></button></div></> : null}
        </article>;
      })}
      {!devices.length ? <div className="rd-empty"><Monitor size={28} /><h2>还没有设备</h2><p>连接一台设备，在这里查看运行状态与使用统计。</p><button className="rd-button" onClick={() => window.remoteCodexHost?.shell.openExternal(`${location.origin}/relay-devices`)}>添加设备<ArrowUpRight size={13} /></button></div> : null}
    </section>
    <section className="rd-statistics" aria-label="使用统计">
      <header className="rd-stats-header"><div><h2>使用统计</h2><p>{timeZone} · 按轮次开始日期统计</p></div><div className="rd-stats-controls"><select aria-label="统计设备" value={selectedDevice} onChange={event => change('device', event.target.value)}><option value="">全部设备</option>{devices.map(d => <option key={d.id} value={d.id}>{d.name}</option>)}</select><div className="rd-segment" role="group" aria-label="统计时间范围">{(['today', '7d', '30d'] as const).map(value => <button key={value} aria-pressed={range === value} onClick={() => change('range', value)}>{value === 'today' ? '今天' : value === '7d' ? '7 天' : '30 天'}</button>)}</div></div></header>
      {(partial || cached.length > 0) ? <p className="rd-data-note" role="status">{partial ? '部分数据 · 旧版或尚未返回统计的设备仅计入索引中可见的最近轮次；tokens 与费用可能不完整。' : ''}{cached.length ? ` ${cached.map(d => `${d.name}：缓存 · ${age(metrics[d.id]?.summaries[range]?.generatedAt ?? d.lastHeartbeatAt, +now)}`).join('；')}` : ''}</p> : null}
      <div className="rd-kpis">{[
        ['总轮数', compact(total.turns), '包含进行中的轮次'],
        ['成功率', total.successRate == null ? '—' : `${(total.successRate * 100).toFixed(1)}%`, '完成 / 全部终态轮次'],
        ['平均轮时长', duration(total.avgDurationMs), '包含工具与等待时间'],
        ['总 tokens', tokenUnknown && total.tokens === 0 ? '—' : compact(total.tokens), `缓存读取 ${compact(total.cachedTokens)} · 已计入输入`],
        ['估算费用', `≈ ${money(total.costUsd)}`, '按模型定价 · 未知费用不推算'],
      ].map(([label, value, hint]) => <div className="rd-kpi" key={label}><span>{label}</span><strong title={label === '总 tokens' ? exact(total.tokens) : label === '总轮数' ? exact(total.turns) : undefined}>{value}</strong><small>{hint}</small></div>)}</div>
      <div className="rd-charts-grid"><section className="rd-chart-panel"><header><h3>每日轮数</h3><span>按设备</span></header><TurnsChart sources={sources} dates={calendarDays(range, timeZone, now)} /></section><section className="rd-chart-panel"><header><h3>{usageMode === 'tokens' ? 'Token 用量' : '估算费用'}</h3><div className="rd-segment rd-segment-small" role="group" aria-label="用量指标"><button aria-pressed={usageMode === 'tokens'} onClick={() => setUsageMode('tokens')}>Tokens</button><button aria-pressed={usageMode === 'cost'} onClick={() => setUsageMode('cost')}>费用</button></div></header>{tokenUnknown && total.tokens === 0 ? <div className="rd-chart-empty">暂无 token 用量数据</div> : <UsageChart daily={total.daily} mode={usageMode} />}</section></div>
      <div className="rd-breakdowns"><section className="rd-chart-panel"><header><h3>模型分布</h3><span>按轮数</span></header>{total.models.length ? <div className="rd-models">{total.models.map((model, i) => <div key={model.model} className="rd-model-row" title={`${model.model}：${exact(model.turns)} 轮 · ${exact(model.tokens)} tokens`}><div><span>{model.model || '未知模型'}</span><b>{exact(model.turns)} <small>轮</small></b></div><div className="rd-bar-track"><span style={{ width: `${model.turns / Math.max(1, total.turns) * 100}%`, background: ['#5287de', '#45a89c', '#bc8bd3', '#cf9054'][i % 4] }} /></div></div>)}</div> : <div className="rd-chart-empty">{hasData ? '当前设备未提供模型数据' : '所选范围暂无模型用量'}</div>}</section>
      <section className="rd-chart-panel"><header><h3>最忙的工作区</h3><span>前 5 名</span></header><div className="rd-ranked-list">{total.workspaces.map((w, i) => <button key={`${w.deviceId}:${w.workspaceId}`} onClick={() => navigate(`/devices/${encodeURIComponent(w.deviceId)}/threads?workspaceId=${encodeURIComponent(w.workspaceId)}`)}><span className="rd-rank">{i + 1}</span><span className="rd-ranked-title" title={w.label}>{w.label}<small>{devices.find(d => d.id === w.deviceId)?.name}</small></span><b>{exact(w.turns)} <small>轮</small></b></button>)}{!total.workspaces.length ? <div className="rd-chart-empty">所选范围暂无工作区活动</div> : null}</div></section>
      <section className="rd-chart-panel"><header><h3>平均用时最长的线程</h3><span>前 5 名</span></header><div className="rd-ranked-list">{total.longest.map((t, i) => <button key={`${t.deviceId}:${t.threadId}`} onClick={() => navigate(`/devices/${encodeURIComponent(t.deviceId)}/threads/${encodeURIComponent(t.threadId)}`)}><span className="rd-rank">{i + 1}</span><span className="rd-ranked-title" title={t.title}>{t.title}<small>{devices.find(d => d.id === t.deviceId)?.name}</small></span><b title={`${exact(t.durationMs)} ms`}>{duration(t.durationMs)}</b></button>)}{!total.longest.length ? <div className="rd-chart-empty">暂无已完成轮次的时长数据</div> : null}</div></section></div>
      <p className="rd-footnote">{partial ? '统计可能不完整。' : '用量与费用仅包含设备上已记录的轮次。'}费用为估算值，实际账单以服务商为准。</p>
    </section>
    {rename ? <FormDialog title="重命名设备" busy={busy} onClose={() => setRename(null)}><form onSubmit={event => { event.preventDefault(); void saveName(); }} className="rd-rename-form"><label htmlFor="rd-device-name">设备名称</label><input id="rd-device-name" autoFocus maxLength={64} value={name} onChange={event => setName(event.target.value)} disabled={busy} />{error ? <p role="alert">{error}</p> : null}<button className="rd-button rd-button-primary" disabled={busy || !name.trim()} type="submit">{busy ? '保存中…' : '保存'}</button></form></FormDialog> : null}
    {update ? <FormDialog title={`${devices.find(d => d.id === update)?.name ?? '设备'} · Runtime`} description="检查版本并管理这台设备的运行环境。" onClose={() => setUpdate(null)}><RuntimeManagement deviceId={update} view="device" /></FormDialog> : null}
  </div>;
}
