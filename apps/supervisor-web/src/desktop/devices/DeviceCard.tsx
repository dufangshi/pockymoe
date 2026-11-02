import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Link } from 'react-router-dom';
import { ArrowRight, ArrowUpRight, ArrowUpCircle, EllipsisVertical, Folder, List } from 'lucide-react';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { ErrorMark, StatusMark, type MapDeviceMenuItem, useLongPress, useMenuKeyboard } from '@pockymoe/thread-ui';
import { DeviceMonitor } from '../../components/DeviceMonitor';
import { DeviceStateMark } from '../../pages/relayAdmin/ui';
import type { DesktopDevice } from '../data/types';
import { Info } from './UsagePanel';
import { Sparkline } from './Charts';
import { compact, deviceColor, newerVersion, type DeviceMetrics, type StatsSummary } from './model';

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
  onUpdate(): void;
  menuItems: MapDeviceMenuItem[];
}
export function DeviceCard({ device, metric, today, history, counts, cacheLabel, partial, loading, skeleton, onUpdate, menuItems }: DeviceCardProps) {
  const { t } = useI18n();
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const anchor = useRef({ x: 0, y: 0 });
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);
  const close = useCallback((restore = false) => { setPosition(null); if (restore) trigger.current?.focus(); }, []);
  const place = (x: number, y: number) => {
    const viewport = window.visualViewport;
    const left = viewport?.offsetLeft ?? 0, top = viewport?.offsetTop ?? 0;
    return { left: Math.max(left + 8, Math.min(x, left + (viewport?.width ?? innerWidth) - 220)), top: Math.max(top + 8, Math.min(y, top + (viewport?.height ?? innerHeight) - 416)) };
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
      <DeviceStateMark showLabel={false} state={!device.connected ? 'offline' : unstable ? 'unstable' : 'online'} label={!device.connected ? t('desktopDevices.offline') : unstable ? t('desktopDevices.unstable') : t('desktopDevices.online')} />
      <Link to={`/desktop/devices/${encodeURIComponent(device.id)}`} className="pd-device-title" title={`${device.name} · Pockymoe ${metric?.version ?? '—'}`}><strong>{device.name}</strong></Link>
      <span className="pd-presence">{device.connected && metric?.latencyMs != null ? <small>{metric.latencyMs}ms</small> : null}</span>
      {update ? <button className="pd-update-badge" disabled={!device.connected} onClick={onUpdate} aria-label={t('desktopDevices.updateAvailable', { version: metric?.latestVersion })} title={t('desktopDevices.updateAvailable', { version: metric?.latestVersion })}>{metric?.version}<ArrowUpRight size={10} aria-hidden /></button> : null}
      <Link className="pd-icon-button pd-workspaces-arrow" to={`/devices/${encodeURIComponent(device.id)}/workspaces`} aria-label={t('desktopDevices.openDeviceWorkspaces', { device: device.name })} title={t('desktopDevices.openDeviceWorkspaces', { device: device.name })}><ArrowRight size={14} aria-hidden /></Link>
      <button ref={trigger} className="pd-icon-button" aria-label={t('desktopDevices.actionsFor', { device: device.name })} aria-haspopup="menu" aria-expanded={Boolean(position)} onClick={() => { if (position) close(); else { const box = trigger.current!.getBoundingClientRect(); show(box.right - 212, box.bottom + 4); } }}><EllipsisVertical size={14} data-icon="more" /></button>
    </header>
    <div className="pd-card-readings"><div className="pd-counts"><span title={t('desktopDevices.running')}><StatusMark status={counts.running ? 'running' : 'idle'} label={counts.running ? t('desktopDevices.running') : t('desktopDevices.noneRunning')} /><b>{counts.running}</b></span><span title={t('desktopDevices.needsYou')}><StatusMark status={counts.attention ? 'waiting' : 'idle'} label={counts.attention ? t('desktopDevices.needsYou') : t('desktopDevices.noneWaiting')} /><b>{counts.attention}</b></span><span title={t('desktopDevices.workspaces', { count: counts.workspaces })}><Folder size={11} aria-hidden /><b>{counts.workspaces}</b><span className="sr-only">{t('desktopDevices.workspaces', { count: counts.workspaces })}</span></span><span title={t('desktopDevices.threads', { count: counts.threads })}><List size={11} aria-hidden /><b>{counts.threads}</b><span className="sr-only">{t('desktopDevices.threads', { count: counts.threads })}</span></span></div><div className="pd-monitor">{device.connected ? <DeviceMonitor key={device.id} deviceId={device.id} /> : null}</div></div>
    <div className="pd-card-bottom"><Sparkline days={history} color={deviceColor(device.id)} label={t('desktopDevices.deviceActivity14d', { device: device.name })} />{loading ? <span className={`pd-today ${skeleton ? 'pd-skeleton' : ''}`} aria-label={t('desktopDevices.loadingToday')} /> : <span className="pd-today" title={t('desktopDevices.today')}><b>{compact(today.totals.turns)}</b>{t('desktopDevices.turnUnit', { count: today.totals.turns })}</span>}{metric?.partial ? <button className="pd-icon-button pd-update-required" aria-label={t('desktopDevices.updateRequired')} title={t('desktopDevices.updateRequired')} disabled={!device.connected} onClick={onUpdate}><ArrowUpCircle size={13} aria-hidden /></button> : null}{cacheLabel || partial ? <Info label={cacheLabel || t('desktopDevices.partialRecent')} message={cacheLabel || t('desktopDevices.partialNote')} /> : null}</div>
    {position && createPortal(<div ref={menu} className="pd-device-menu" role="menu" aria-label={t('desktopDevices.actionsMenuFor', { device: device.name })} style={position}>
      {menuItems.map(item => <div key={item.id} className={item.separator ? 'pd-menu-separated' : undefined}>
        {item.separator ? <span role="separator" className="sr-only" /> : null}
        <button role="menuitem" disabled={item.disabled} data-tone={item.tone} onClick={() => choose(item.onSelect)}>{item.label}</button>
        {item.disabledReason ? <ErrorMark kind="unavailable" message={item.disabledReason} /> : null}
      </div>)}
    </div>, document.body)}
  </article>;
}
