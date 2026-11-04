import './navigation.css';
import { LayoutDashboard, Monitor } from 'lucide-react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useLocation, useNavigate } from 'react-router-dom';
import { useDesktopSelector } from './data';
import { buildActivityItems } from './data/activity';

/** Global entries have one order and selected state in the client and browser. */
export function ShellRail({ openMap, mapVisible = false, mapShortcut }: { openMap(): void; mapVisible?: boolean; mapShortcut?: string }) {
  useI18n();
  const navigate = useNavigate(), location = useLocation();
  const waiting = useDesktopSelector(index => new Set(buildActivityItems(index).filter(item => item.state === 'needsInput').map(item => item.rootThreadKey ?? item.key)).size);
  const devices = location.pathname.startsWith('/desktop/devices');
  return <><button data-desktop-map-link aria-label={waiting ? translate('desktopShell.mapWaiting', { count: waiting }) : translate('desktopShell.map')} aria-pressed={mapVisible} title={mapShortcut ? translate('desktopShell.mapShortcut', { shortcut: mapShortcut }) : translate('desktopShell.map')} onClick={openMap}><LayoutDashboard data-icon="thread-map" />{waiting ? <span className="desktop-map-badge">{waiting}</span> : null}</button><button aria-label={translate('desktopShell.devices')} title={translate('desktopShell.devices')} aria-pressed={devices} onClick={() => navigate('/desktop/devices')}><Monitor data-icon="devices" /></button></>;
}
