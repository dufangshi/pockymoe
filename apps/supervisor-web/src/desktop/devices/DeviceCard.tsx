import { useUsageColors } from './UsageColors';
import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { ArrowRight, ArrowUpRight, ArrowUpCircle, Folder, List } from 'lucide-react';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { StatusMark } from '@pockymoe/thread-ui';
import { DeviceMonitor } from '../../components/DeviceMonitor';
import { FinishProtectionButton } from '../../components/DeviceSetupProgress';
import { fetchAgentIsolation } from '../../lib/api';
import { DeviceStateMark } from '../../components/DeviceStateMark';
import { IsolationMark } from '../../components/managed/IsolationMark';
import { ManagedObjectMenu, useManagedObjectMenu, type ManagedAction } from '../../components/managed/ManagedObjectMenu';
import type { DesktopDevice } from '../data/types';
import { Info } from './UsagePanel';
import { Sparkline } from './Charts';
import { compact, newerVersion, type DeviceMetrics, type StatsSummary } from './model';

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
  menuItems: ManagedAction[];
  /** Changes when the device's settings may have changed, to read its isolation again. */
  isolationRevision?: number;
  /** This browser started the device's protection and it is not finished: the
      device is online but refuses work until the reader finishes it. */
  protectionPending?: boolean;
  /** On the device's own page: its readings only. The page's header already names
      the device, marks its state and holds its one menu (issue #116). */
  bare?: boolean;
}
export function DeviceCard({ device, metric, today, history, counts, cacheLabel, partial, loading, skeleton, onUpdate, menuItems, isolationRevision = 0, protectionPending = false, bare = false }: DeviceCardProps) {
  const { t } = useI18n();
  // Isolation is the device's own setting, read from it while it is online; a
  // device shared with you (owner only) or an older one shows no mark.
  const [isolated, setIsolated] = useState(false);
  useEffect(() => {
    if (!device.connected || bare) return;
    let alive = true;
    fetchAgentIsolation(device.id).then(value => { if (alive) setIsolated(Boolean(value.device)); }).catch(() => {});
    return () => { alive = false; };
  }, [device.id, device.connected, isolationRevision, bare]);
  const { deviceColor } = useUsageColors();
  // The same menu from the ⋯ key, a right click and a long press on the card (§3).
  const menu = useManagedObjectMenu();
  const unstable = (device as DesktopDevice & { unstable?: boolean }).unstable;
  const update = newerVersion(metric?.version, metric?.latestVersion);
  const updateBadge = update ? <button className="pd-update-badge" disabled={!device.connected} onClick={onUpdate} aria-label={t('desktopDevices.updateAvailable', { version: metric?.latestVersion })} title={t('desktopDevices.updateAvailable', { version: metric?.latestVersion })}>{metric?.version}<ArrowUpRight size={10} aria-hidden /></button> : null;
  return <article className={`pd-device-card ${device.connected ? '' : 'is-offline'} ${bare ? 'is-bare' : ''}`} aria-label={device.name} {...(bare ? {} : menu.rowProps)}>
    {bare ? (protectionPending && device.connected) || update ? <header className="pd-card-heading">
      {protectionPending && device.connected ? <FinishProtectionButton compact deviceId={device.id} /> : null}
      {updateBadge}
    </header> : null : <header className="pd-card-heading">
      <DeviceStateMark showLabel={false} state={!device.connected ? 'offline' : unstable ? 'unstable' : 'online'} label={!device.connected ? t('desktopDevices.offline') : unstable ? t('desktopDevices.unstable') : t('desktopDevices.online')} />
      <Link to={`/desktop/devices/${encodeURIComponent(device.id)}`} className="pd-device-title" title={`${device.name} · Pockymoe ${metric?.version ?? '—'}`}><strong>{device.name}</strong></Link>
      {isolated ? <IsolationMark kind="device" /> : null}
      {protectionPending && device.connected ? <FinishProtectionButton compact deviceId={device.id} /> : null}
      <span className="pd-presence">{device.connected && metric?.latencyMs != null ? <small>{metric.latencyMs}ms</small> : null}</span>
      {updateBadge}
      <Link className="pd-icon-button pd-workspaces-arrow" to={`/devices/${encodeURIComponent(device.id)}/workspaces`} aria-label={t('desktopDevices.openDeviceWorkspaces', { device: device.name })} title={t('desktopDevices.openDeviceWorkspaces', { device: device.name })}><ArrowRight size={14} aria-hidden /></Link>
      <ManagedObjectMenu ref={menu.ref} name={device.name} items={menuItems} className="pd-icon-button" />
    </header>}
    <div className="pd-card-readings"><div className="pd-counts"><span title={t('desktopDevices.running')}><StatusMark status={counts.running ? 'running' : 'idle'} label={counts.running ? t('desktopDevices.running') : t('desktopDevices.noneRunning')} /><b>{counts.running}</b></span><span title={t('desktopDevices.needsYou')}><StatusMark status={counts.attention ? 'waiting' : 'idle'} label={counts.attention ? t('desktopDevices.needsYou') : t('desktopDevices.noneWaiting')} /><b>{counts.attention}</b></span><span title={t('desktopDevices.workspaces', { count: counts.workspaces })}><Folder size={11} aria-hidden /><b>{counts.workspaces}</b><span className="sr-only">{t('desktopDevices.workspaces', { count: counts.workspaces })}</span></span><span title={t('desktopDevices.threads', { count: counts.threads })}><List size={11} aria-hidden /><b>{counts.threads}</b><span className="sr-only">{t('desktopDevices.threads', { count: counts.threads })}</span></span></div><div className="pd-monitor">{device.connected ? <DeviceMonitor key={device.id} deviceId={device.id} /> : null}</div></div>
    <div className="pd-card-bottom"><Sparkline days={history} color={deviceColor(device.id)} label={t('desktopDevices.deviceActivity14d', { device: device.name })} />{loading ? <span className={`pd-today ${skeleton ? 'pd-skeleton' : ''}`} aria-label={t('desktopDevices.loadingToday')} /> : <span className="pd-today" title={t('desktopDevices.today')}><b>{compact(today.totals.turns)}</b>{t('desktopDevices.turnUnit', { count: today.totals.turns })}</span>}{metric?.partial ? <button className="pd-icon-button pd-update-required" aria-label={t('desktopDevices.updateRequired')} title={t('desktopDevices.updateRequired')} disabled={!device.connected} onClick={onUpdate}><ArrowUpCircle size={13} aria-hidden /></button> : null}{cacheLabel || partial ? <Info label={cacheLabel || t('desktopDevices.partialRecent')} message={cacheLabel || t('desktopDevices.partialNote')} /> : null}</div>
  </article>;
}
