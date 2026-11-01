import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Plus, Settings } from 'lucide-react';
import { ErrorMark, MatterWorkbench, ThreadMap, type MapDeviceExtras, type MapScope, type ThreadMapDevice } from '@pockymoe/thread-ui';
import { useNavigate } from 'react-router-dom';
import { useAppShellNav } from '../components/AppShellNavContext';
import { ConversationSearch } from '../components/ConversationSearch';
import { mapDevice } from '../components/ThreadMapHost';
import { RecentThreadMenu } from '../components/RecentThreadMenu';
import { useWorkbenchNavigation } from '../pages/useWorkbenchNavigation';
import { workbenchUrls } from '../lib/workbenchUrls';
import { appPathname, appUrl } from '../lib/webBase';
import { relayFetchInit, relayUrl } from '../lib/relayOrigin';
import { threadHref, workspacesHref } from '../lib/relayRoutes';
import { deviceRequest } from '../lib/api';
import { useDesktopSelector, useDesktopStore, failed, unread } from './data';
import { attentionFromActivity, buildActivityItems } from './data/activity';
import { useDesktopWorkbench } from './context';
import { resolveRequest } from './integration';
import { QuickLook } from './QuickLook';
import { deviceCounts } from './devices/model';

export function DesktopMapPage({ visible, mapVisible, revealKey, onOpen, children }: { visible: boolean; mapVisible: boolean; revealKey: string | null; onOpen(key: string, newWindow?: boolean): void; children?: ReactNode }) {
  const index = useDesktopSelector(index => index); const store = useDesktopStore();
  const shell = useAppShellNav(); const navigate = useNavigate();
  const [scope, setScope] = useState<MapScope>('week'); const [filter, setFilter] = useState('');
  const [quickLook, setQuickLook] = useState<string | null>(null); const [error, setError] = useState('');
  const [renameSupported, setRenameSupported] = useState<ReadonlySet<string>>(new Set());
  const [searchOpen, setSearchOpen] = useState(false);
  const first = index.devices.find(device => device.connected) ?? index.devices[0];
  const firstThreads = useMemo(() => index.threads.filter(thread => thread.deviceId === first?.id), [first?.id, index.threads]);
  const navigation = useWorkbenchNavigation(null, firstThreads, first?.id ?? null, undefined, { sharesDeviceThreads: true, enabled: visible });
  const desktopWorkbench = useDesktopWorkbench(navigation);
  const previous = useRef(new Map<string, ThreadMapDevice>());
  const devices = useMemo(() => index.devices.map(device => {
    const workspaces = index.workspaces.filter(workspace => workspace.deviceId === device.id), threads = index.threads.filter(thread => thread.deviceId === device.id);
    const activity = index.activity[device.id] ?? null;
    const cached = previous.current.get(device.id);
    const candidate = mapDevice(device.id, device.name, workspaces, threads, activity);
    const decorate = (tile: typeof candidate.workspaces[number]['threads'][number]) => {
      const source = index.threads.find(thread => thread.key === tile.key);
      if (source) {
        tile.unread = !failed(source) && source.status !== 'interrupted' && unread(source);
        if (source.requests[0]) tile.needsInput = { kind: source.requests[0].kind, summary: source.requests[0].summary, requestId: source.requests[0].id };
      }
      tile.children?.forEach(decorate);
    };
    candidate.workspaces.forEach(workspace => workspace.threads.forEach(decorate));
    candidate.state = device.connected ? device.cachedAt ? 'ready' : 'loading' : 'offline';
    if (device.cachedAt) candidate.cachedAt = device.cachedAt;
    // The shared map owns tile layout; unchanged device subtrees retain identity.
    if (cached && JSON.stringify(cached) === JSON.stringify(candidate)) return cached;
    previous.current.set(device.id, candidate); return candidate;
  }), [index.activity, index.devices, index.threads, index.workspaces]);
  const attention = useMemo(() => attentionFromActivity(buildActivityItems(index)), [index]);
  const deviceIds = index.devices.map(device => device.id).sort().join(',');
  useEffect(() => {
    if (!visible) { setSearchOpen(false); setQuickLook(null); return; }
    let active = true;
    for (const id of deviceIds.split(',').filter(Boolean)) void fetch(relayUrl(`relay/devices/${encodeURIComponent(id)}`), relayFetchInit(`relay/devices/${encodeURIComponent(id)}`, { method: 'OPTIONS', credentials: 'include' })).then(response => {
      if (active && /\bPATCH\b/i.test(response.headers.get('allow') ?? '')) setRenameSupported(previous => new Set([...previous, id]));
    }).catch(() => undefined);
    return () => { active = false; };
  }, [visible, deviceIds]);
  useLayoutEffect(() => {
    if (mapVisible && devices.some(device => device.workspaces.length) && !performance.getEntriesByName('pockymoe-desktop-map-paint').length) performance.measure('pockymoe-desktop-map-paint', 'pockymoe-desktop-shell-mount');
  }, [devices, mapVisible]);
  const extras = useMemo(() => Object.fromEntries(index.devices.map(device => {
    const { metrics, ...extra } = index.extras[device.id] ?? {}; const counts = deviceCounts(index, device.id);
    const compactMetrics = metrics ? [`CPU ${metrics.cpu.usagePercent?.toFixed(0) ?? '—'}%`, `内存 ${metrics.memory.usagePercent?.toFixed(0) ?? '—'}%`, ...(metrics.gpus.length ? [`GPU ${metrics.gpus[0]?.usagePercent?.toFixed(0) ?? '—'}%`] : [])].join(' · ') : undefined;
    const value: MapDeviceExtras = { ...extra, ...(compactMetrics ? { metrics: compactMetrics } : {}), runningCount: counts.running, waitingCount: counts.attention,
      menuItems: [
        { id: 'details', label: '设备详情', onSelect: () => navigate(`/desktop/devices/${encodeURIComponent(device.id)}`) },
        { id: 'check', label: '检查更新', disabled: !device.connected, onSelect: () => { void deviceRequest(device.id, 'api/management/supervisor', { method: 'POST', body: JSON.stringify({ action: 'check' }) }).then(() => navigate(`/desktop/devices/${encodeURIComponent(device.id)}?action=update`)).catch(error => setError(error instanceof Error ? error.message : String(error))); } },
        ...(renameSupported.has(device.id) ? [{ id: 'rename', label: '重命名', onSelect: () => navigate(`/desktop/devices/${encodeURIComponent(device.id)}?action=rename`) }] : []),
        { id: 'browser', label: '在浏览器中管理', onSelect: () => window.pockymoeHost?.shell.openExternal(new URL(appUrl(workspacesHref(device.id)), location.origin).toString()) },
      ],
    };
    return [device.id, value];
  })), [index, navigate, renameSupported]);
  const decide = useCallback((key: string, requestId: string, decision: 'approve' | 'deny') => {
    const thread = store.getSnapshot().threads.find(thread => thread.key === key); if (!thread) return;
    void resolveRequest(thread.deviceId, thread.id, requestId, decision, { store }).catch(error => setError(error instanceof Error ? error.message : String(error)));
  }, [store]);
  return <div className="desktop-map-scene thread-ui-shell thread-ui-viewport-constrained" hidden={!visible} inert={!visible} data-theme-effective={shell?.effectiveTheme} data-theme-mode={shell?.themeMode}>
    <MatterWorkbench title="地图" homeHref={appUrl('/')} options={workbenchUrls({ ...navigation, ...desktopWorkbench, emptyWorkspace: !visible, activeView: 'chat', terminalEnabled: false, onViewChange: () => {}, onNavigate: navigate, onSearch: () => setSearchOpen(true), searchOpen,
      search: <ConversationSearch threadId={firstThreads[0]?.id ?? ''} workspaceId={firstThreads[0]?.workspaceId ?? ''} deviceLabel={first?.name ?? '设备'} deviceId={first?.id} initialScope="device" allowGlobal turns={[]} open={searchOpen} onOpen={() => setSearchOpen(true)} onClose={() => setSearchOpen(false)} onSelect={(_turns, turnId, itemId, query) => { if (firstThreads[0]) { setSearchOpen(false); navigate(`${threadHref(firstThreads[0].id, first?.id)}?${new URLSearchParams({ turnId, itemId, query })}`); } }} onNavigate={(match, query) => { setSearchOpen(false); navigate(`${threadHref(match.threadId, first?.id)}?${new URLSearchParams({ query, ...(match.turnId ? { turnId: match.turnId } : {}), ...(match.itemId ? { itemId: match.itemId } : {}) })}`); }} />,
      renderThreadMenu: (thread, context) => <RecentThreadMenu thread={thread} context={context} currentKey={navigation.currentKey} onFavorite={navigation.onToggleThreadFavorite} onRenamed={navigation.onThreadRenamed} onRemoved={navigation.onThreadRemoved} onNavigate={navigate} />,
    })} settings={<button aria-label="设置" onClick={shell?.openSettings}><Settings data-icon="settings" /></button>} newThread={<button aria-label="新线程" onClick={() => first && navigate(`/devices/${encodeURIComponent(first.id)}/threads/new`)}><Plus data-icon="new-thread" /></button>} actions={null} threadMenu={null} connection={<span className="desktop-online-count">{index.devices.filter(device => device.connected).length} 台在线</span>} explorer={null} revealExplorer={0}>
      <div className="desktop-map-persistent" hidden={!mapVisible} inert={!mapVisible}><ThreadMap devices={devices} presentation="page" stableLayout scope={scope} onScopeChange={setScope} filterText={filter} onFilterChange={setFilter} attention={attention} deviceExtras={extras} revealKey={revealKey} pulseKeys={index.pulseKeys} onQuickLook={setQuickLook} onRequestDecision={decide} onOpen={thread => onOpen(thread.key)} onNavigate={href => navigate(appPathname(href))} onRetry={id => { void store.refreshDevice(id); }} actions={error ? <ErrorMark message={error} retryLabel="关闭提示" onRetry={() => setError('')} /> : index.error ? <ErrorMark message={index.error} onRetry={() => { void store.refresh(); }} /> : undefined} /></div>
      {!mapVisible ? <div className="desktop-map-route">{children}</div> : null}
      {quickLook && mapVisible ? <QuickLook threadKey={quickLook} onClose={() => setQuickLook(null)} onOpen={(key, newWindow) => { setQuickLook(null); onOpen(key, newWindow); }} /> : null}
    </MatterWorkbench>
  </div>;
}
