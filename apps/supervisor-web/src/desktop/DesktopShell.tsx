import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { LayoutDashboard, Monitor } from 'lucide-react';
import { useLocation, useNavigate } from 'react-router-dom';
import { AppShellNavContext, useAppShellNav } from '../components/AppShellNavContext';
import { appPathname, appUrl } from '../lib/webBase';
import { threadHref } from '../lib/relayRoutes';
import { useThemeRegistry } from '../lib/themePacks';
import { useFontSize } from '../lib/fontSize';
import { useDesktopSelector, useDesktopStore } from './data';
import { buildActivityItems } from './data/activity';
import { ActivitySection } from './ActivitySection';
import { DesktopSurfaceContext, type NavigationActions } from './context';
import { DesktopMapPage } from './MapPage';
import { focusedKeyFromPath, handleHostCommand, syncActivity } from './integration';
import type { HostCapabilities } from './host';
import './desktop.css';

export function DesktopShell({ children, capabilities }: { children: ReactNode; capabilities: HostCapabilities }) {
  const index = useDesktopSelector(index => index); const store = useDesktopStore();
  const location = useLocation(); const navigate = useNavigate(); const shell = useAppShellNav();
  const host = window.pockymoeHost!; const registry = useThemeRegistry(); const [fontSize] = useFontSize();
  const mapVisible = location.pathname === '/';
  const devicesPage = location.pathname === '/desktop/devices' || location.pathname.startsWith('/desktop/devices/');
  const [revealKey, setRevealKey] = useState<string | null>(null);
  const route = useRef(location.pathname); route.current = location.pathname;
  const openMap = useCallback(() => navigate('/'), [navigate]);
  const revealInMap = useCallback((key: string) => { setRevealKey(null); navigate('/'); requestAnimationFrame(() => setRevealKey(key)); }, [navigate]);
  const open = useCallback((target: string, newWindow = false) => {
    const thread = store.getSnapshot().threads.find(thread => thread.key === target);
    const path = thread ? threadHref(thread.id, thread.deviceId) : appPathname(target);
    if (newWindow && capabilities.features.multiWindow && typeof host.window?.openThread === 'function') host.window.openThread(appUrl(path), { newWindow: true });
    else navigate(path);
  }, [capabilities.features.multiWindow, host, navigate, store]);
  const newThread = useCallback(() => {
    const currentDevice = route.current.match(/^\/devices\/([^/]+)/)?.[1];
    const device = currentDevice ? decodeURIComponent(currentDevice) : store.getSnapshot().devices.find(device => device.connected)?.id;
    navigate(device ? `/devices/${encodeURIComponent(device)}/threads/new` : '/desktop/devices');
  }, [navigate, store]);
  useEffect(() => { store.setMapVisible(mapVisible); return () => store.setMapVisible(false); }, [mapVisible, store]);
  useEffect(() => {
    window.dispatchEvent(new Event('pockymoe-desktop-route'));
    const key = focusedKeyFromPath(location.pathname);
    const thread = store.getSnapshot().threads.find(thread => thread.key === key);
    host.window?.setTitle(thread?.title ?? (mapVisible ? '地图 · Pockymoe' : 'Pockymoe'));
    if (!key) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const focus = () => { clearTimeout(timer); if (document.hasFocus()) timer = setTimeout(() => store.markRead(key), 1500); };
    const scroll = (event: Event) => {
      const target = event.target;
      if (target instanceof HTMLElement && target.matches('[data-testid="thread-scroll-container"], .thread-graph-scroll-container') && document.hasFocus() && target.scrollHeight - target.scrollTop - target.clientHeight < 8) store.markRead(key);
    };
    focus(); window.addEventListener('focus', focus); window.addEventListener('blur', focus); document.addEventListener('scroll', scroll, true);
    return () => { clearTimeout(timer); window.removeEventListener('focus', focus); window.removeEventListener('blur', focus); document.removeEventListener('scroll', scroll, true); };
  }, [host, location.pathname, mapVisible, store]);
  useEffect(() => syncActivity(store, host, () => focusedKeyFromPath(route.current), capabilities.features.badge), [capabilities.features.badge, host, store]);
  useEffect(() => {
    if (typeof host.onCommand !== 'function') return;
    return host.onCommand(command => { void handleHostCommand(command, { store, navigate, openMap, revealInMap, newThread }).then(result => host.commandResult(command.id, result)); });
  }, [host, navigate, newThread, openMap, revealInMap, store]);
  useEffect(() => {
    if (typeof host.appearance?.sync !== 'function') return;
    const frame = requestAnimationFrame(() => {
      const root = Array.from(document.querySelectorAll<HTMLElement>('.thread-ui-shell')).find(node => node.getClientRects().length) ?? document.documentElement;
      const accent = getComputedStyle(root).getPropertyValue('--theme-accent-solid').trim();
      host.appearance.sync({ effective: shell?.effectiveTheme ?? 'light', pack: registry.applied, accent, fontScale: fontSize / 16 });
    });
    return () => cancelAnimationFrame(frame);
  }, [fontSize, host, registry.applied, shell?.effectiveTheme]);
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey)) return;
      if (event.key === '0') { event.preventDefault(); openMap(); }
      else if (event.key.toLowerCase() === 'k') {
        event.preventDefault();
        Array.from(document.querySelectorAll<HTMLButtonElement>('.matter-search-trigger')).find(button => button.getClientRects().length)?.click();
      }
    };
    window.addEventListener('keydown', key); return () => window.removeEventListener('keydown', key);
  }, [openMap]);
  const waitingCount = useMemo(() => new Set(buildActivityItems(index).filter(item => item.state === 'needsInput').map(item => item.rootThreadKey ?? item.key)).size, [index]);
  const railLeading = useMemo(() => <><button data-desktop-map-link aria-label={`地图${waitingCount ? `，${waitingCount} 个需要你` : ''}`} aria-pressed={mapVisible} title="地图 (⌘0)" onClick={openMap}><LayoutDashboard data-icon="thread-map" />{waitingCount ? <span className="desktop-map-badge">{waitingCount}</span> : null}</button><button aria-label="设备" title="设备" onClick={() => navigate('/desktop/devices')}><Monitor data-icon="devices" /></button></>, [mapVisible, navigate, openMap, waitingCount]);
  const surface = useMemo(() => ({ multiWindow: capabilities.features.multiWindow, openMap, revealInMap, railLeading, renderActivity: (navigation: NavigationActions) => <ActivitySection navigation={navigation} onOpen={open} /> }), [capabilities.features.multiWindow, openMap, revealInMap, railLeading, open]);
  const chrome = capabilities.windowChrome;
  const style = { '--desktop-title-height': `${chrome.toolbarHeight}px`, '--desktop-controls-left': chrome.controlsSide === 'left' ? `${chrome.trafficLightsWidth}px` : '0px', '--desktop-controls-right': chrome.controlsSide === 'right' ? `${chrome.trafficLightsWidth}px` : '0px' } as CSSProperties;
  const shellValue = useMemo(() => shell ? { ...shell, openThreadMap: openMap } : shell, [shell, openMap]);
  return <DesktopSurfaceContext.Provider value={surface}><AppShellNavContext.Provider value={shellValue}><div className="desktop-surface" data-desktop-surface style={style}>
    <DesktopMapPage visible={mapVisible || devicesPage} mapVisible={mapVisible} revealKey={revealKey} onOpen={open}>{devicesPage ? children : null}</DesktopMapPage>
    {!mapVisible && !devicesPage ? <div className="desktop-route">{children}</div> : null}
  </div></AppShellNavContext.Provider></DesktopSurfaceContext.Provider>;
}
