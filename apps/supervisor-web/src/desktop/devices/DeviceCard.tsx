import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Link } from 'react-router-dom';
import { ArrowUpRight, EllipsisVertical, Monitor } from 'lucide-react';
import { StatusMark, useLongPress, useMenuKeyboard } from '@pockymoe/thread-ui';
import { DeviceMonitor } from '../../components/DeviceMonitor';
import { DeviceStateMark } from '../../pages/relayAdmin/ui';
import type { DesktopDevice } from '../data/types';
import { Sparkline } from './Charts';
import { compact, deviceColor, money, newerVersion, type DeviceMetrics, type StatsSummary } from './model';

export interface DeviceCardProps {
  device: DesktopDevice;
  metric: DeviceMetrics | undefined;
  today: StatsSummary;
  history: StatsSummary['daily'];
  counts: { running: number; attention: number; workspaces: number; threads: number };
  cacheLabel: string;
  partial: boolean;
  loading: boolean;
  skeleton: boolean;
  renameSupported: boolean;
  onReveal(): void;
  onNewThread(): void;
  onRename(): void;
  onUpdate(): void;
  onBrowser(): void;
}
export function DeviceCard({ device, metric, today, history, counts, cacheLabel, partial, loading, skeleton, renameSupported, onReveal, onNewThread, onRename, onUpdate, onBrowser }: DeviceCardProps) {
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const anchor = useRef({ x: 0, y: 0 });
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);
  const close = useCallback((restore = false) => { setPosition(null); if (restore) trigger.current?.focus(); }, []);
  const place = (x: number, y: number) => {
    const viewport = window.visualViewport;
    const left = viewport?.offsetLeft ?? 0, top = viewport?.offsetTop ?? 0;
    return { left: Math.max(left + 8, Math.min(x, left + (viewport?.width ?? innerWidth) - 220)), top: Math.max(top + 8, Math.min(y, top + (viewport?.height ?? innerHeight) - 292)) };
  };
  const show = (x: number, y: number) => {
    const box = trigger.current!.getBoundingClientRect(); anchor.current = { x: x - box.left, y: y - box.top };
    setPosition(place(x, y));
  };
  const longPress = useLongPress(point => show(point.x, point.y));
  useMenuKeyboard(menu, Boolean(position), close);
  useEffect(() => {
    if (!position) return;
    const outside = (event: Event) => { if (!menu.current?.contains(event.target as Node) && !trigger.current?.contains(event.target as Node)) close(); };
    const reposition = () => {
      const box = trigger.current?.getBoundingClientRect();
      const viewport = window.visualViewport;
      const left = viewport?.offsetLeft ?? 0, top = viewport?.offsetTop ?? 0;
      if (!box || box.bottom < top || box.top > top + (viewport?.height ?? innerHeight) || box.right < left || box.left > left + (viewport?.width ?? innerWidth)) { close(); return; }
      setPosition(place(box.left + anchor.current.x, box.top + anchor.current.y));
    };
    const scroll = (event: Event) => { if (!menu.current?.contains(event.target as Node)) reposition(); };
    const resize = () => reposition();
    document.addEventListener('pointerdown', outside);
    window.addEventListener('scroll', scroll, true); window.addEventListener('resize', resize);
    window.visualViewport?.addEventListener('resize', resize); window.visualViewport?.addEventListener('scroll', resize);
    return () => { document.removeEventListener('pointerdown', outside); window.removeEventListener('scroll', scroll, true); window.removeEventListener('resize', resize); window.visualViewport?.removeEventListener('resize', resize); window.visualViewport?.removeEventListener('scroll', resize); };
  }, [close, position]);
  const choose = (action: () => void) => { close(true); action(); };
  const unstable = (device as DesktopDevice & { unstable?: boolean }).unstable;
  const update = newerVersion(metric?.version, metric?.latestVersion);
  return <article className={`pd-device-card ${device.connected ? '' : 'is-offline'}`} aria-label={device.name} {...longPress} onContextMenu={event => { event.preventDefault(); show(event.clientX, event.clientY); }}>
    <header className="pd-card-heading">
      <Link to={`/desktop/devices/${encodeURIComponent(device.id)}`} className="pd-device-title"><Monitor size={16} aria-hidden /><strong>{device.name}</strong></Link>
      <span className="pd-presence"><DeviceStateMark state={!device.connected ? 'offline' : unstable ? 'unstable' : 'online'} label={!device.connected ? '离线' : unstable ? '频繁重连' : '在线'} />{device.connected && metric?.latencyMs != null ? <small>{metric.latencyMs}ms</small> : null}</span>
      <button ref={trigger} className="pd-icon-button" aria-label={`${device.name} 的操作`} aria-haspopup="menu" aria-expanded={Boolean(position)} onClick={() => { if (position) close(); else { const box = trigger.current!.getBoundingClientRect(); show(box.right - 212, box.bottom + 4); } }}><EllipsisVertical size={16} data-icon="more" /></button>
    </header>
    <div className="pd-version"><span>Pockymoe <b>{metric?.version ?? '—'}</b></span>{update ? <button disabled={!device.connected} onClick={onUpdate}>有更新 {metric?.latestVersion}<ArrowUpRight size={11} aria-hidden /></button> : null}</div>
    <div className="pd-counts"><span><span className="pd-count-mark"><StatusMark status={counts.running ? 'running' : 'idle'} label={counts.running ? '运行中' : '没有运行中的线程'} /></span><b>{counts.running}</b> 运行中</span><span><span className="pd-count-mark"><StatusMark status={counts.attention ? 'waiting' : 'idle'} label={counts.attention ? '需要你' : '没有等待输入的线程'} /></span><b>{counts.attention}</b> 需要你</span><span><b>{counts.workspaces}</b> 工作区</span><span><b>{counts.threads}</b> 线程</span></div>
    {loading ? <div className={`pd-today ${skeleton ? 'pd-skeleton' : ''}`} aria-label="正在读取今日统计" /> : <div className="pd-today"><span>今日</span><b>{compact(today.totals.turns)}</b> 轮 <span>·</span> {partial ? '—' : compact(today.totals.inputTokens + today.totals.outputTokens)} tokens <span>·</span> ≈ {money(today.totals.costUsd)}</div>}
    <div className="pd-card-bottom"><Sparkline days={history} color={deviceColor(device.id)} label={`${device.name} 近 14 天活动`} /><span>{cacheLabel || (partial ? '部分数据 · 最近轮次' : '近 14 天活动')}</span></div>
    <footer className="pd-monitor"><span>实时资源</span>{device.connected ? <DeviceMonitor key={device.id} deviceId={device.id} /> : <small>设备离线，监控暂停</small>}</footer>
    {position && createPortal(<div ref={menu} className="pd-device-menu" role="menu" aria-label={`${device.name} 的操作菜单`} style={position}>
      <button role="menuitem" onClick={() => choose(onReveal)}>在地图中显示</button>
      <Link role="menuitem" to={`/desktop/devices/${encodeURIComponent(device.id)}`} onClick={() => close()}>设备详情</Link>
      <button role="menuitem" disabled={!device.connected} onClick={() => choose(onNewThread)}>新建线程</button>
      <button role="menuitem" disabled={!device.connected} onClick={() => choose(onUpdate)}>检查更新</button>
      {renameSupported ? <button role="menuitem" onClick={() => choose(onRename)}>重命名</button> : null}
      <button role="menuitem" onClick={() => choose(onBrowser)}>在浏览器中管理<ArrowUpRight size={13} aria-hidden /></button>
    </div>, document.body)}
  </article>;
}
