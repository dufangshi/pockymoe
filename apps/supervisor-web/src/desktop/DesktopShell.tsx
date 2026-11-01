import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { ChevronDown, ChevronLeft, ChevronRight, CircleHelp, Command, Monitor, PanelLeft, PanelRight, Plus, Search, Settings, Sparkles } from 'lucide-react';
import { useLocation, useNavigate } from 'react-router-dom';
import { useAppShellNav } from '../components/AppShellNavContext';
import { deviceRequest } from '../lib/api';
import type { HostCapabilities } from './host';
import { useDesktopSelector, useDesktopStore } from './data';
import { threadPath } from './data/types';
import { focusedKeyFromPath, handleHostCommand, syncActivity } from './activity';
import { DesktopTree } from './nav/Tree';
import { CommandPalette, type PaletteAction } from './nav/CommandPalette';
import { useNavPreferences } from './nav/preferences';
import { smartThreads } from './nav/model';
import { StatusGlyph } from './nav/StatusGlyph';
import { LocationPicker } from './nav/LocationPicker';
import { menuKeyboard } from './nav/keyboard';
import './desktop.css';
import { buildDeviceActions } from './devices/actions';
const DevicesPage = lazy(() => import('./devices'));


function DesktopHome({ onOpen, onNew }: { onOpen(path: string): void; onNew(): void }) {
  const index = useDesktopSelector(index => index);
  const recent = [...index.threads].sort((a, b) => (b.lastViewedAt ?? b.updatedAt).localeCompare(a.lastViewedAt ?? a.updatedAt)).slice(0, 6);
  return <div className="rc-home"><div className="rc-home-heading"><span className="rc-brand-mark"><Command size={23} /></span><p>REMOTE CODEX</p><h1>继续你的工作</h1><span>所有设备，一个工作空间。</span></div><button className="rc-new-card" onClick={onNew}><Plus size={18} /><span>新建线程<small>从一个想法开始</small></span><kbd>⌘ N</kbd></button><div className="rc-home-section"><span>最近的线程</span><span>{index.devices.filter(device => device.connected).length} 台设备在线</span></div><div className="rc-recent-list">{recent.map(thread => <button key={thread.key} onClick={() => onOpen(threadPath(thread))}><StatusGlyph thread={thread} /><span>{thread.title}<small>{index.devices.find(device => device.id === thread.deviceId)?.name} · {index.workspaces.find(workspace => workspace.deviceId === thread.deviceId && workspace.id === thread.workspaceId)?.label}</small></span><ChevronRight size={14} /></button>)}</div><div className="rc-home-hint"><kbd>⌘ K</kbd> 搜索线程、工作区和命令</div></div>;
}
export function DesktopShell({ capabilities, children }: { capabilities: HostCapabilities; children: ReactNode }) {
  const store = useDesktopStore(); const index = useDesktopSelector(index => index);
  const navigate = useNavigate(); const location = useLocation(); const shell = useAppShellNav();
  const host = window.remoteCodexHost!;
  const [preferences, updatePreferences] = useNavPreferences(index.accountId);
  const [choosingLocation, setChoosingLocation] = useState(false);
  const [palette, setPalette] = useState(false); const [breadcrumbMenu, setBreadcrumbMenu] = useState<string | null>(null);
  const [inspector, setInspector] = useState(false);
  const [focusedWindow, setFocusedWindow] = useState(document.hasFocus());
  const route = useRef(location.pathname); route.current = location.pathname;
  const content = useRef<HTMLDivElement>(null);
  const scrollPositions = useRef(new Map<string, number>());
  const history = useRef({ paths: [location.pathname + location.search], cursor: 0, traversing: false });
  const [historyVersion, setHistoryVersion] = useState(0);
  const focusedKey = focusedKeyFromPath(location.pathname);
  const thread = index.threads.find(thread => thread.key === focusedKey);
  const currentDeviceId = thread?.deviceId ?? decodeURIComponent(location.pathname.match(/^\/devices\/([^/]+)/)?.[1] ?? '');
  const device = index.devices.find(device => device.id === currentDeviceId);
  const workspace = index.workspaces.find(workspace => workspace.deviceId === currentDeviceId && workspace.id === (thread?.workspaceId ?? new URLSearchParams(location.search).get('workspaceId')));
  const open = useCallback((path: string, newWindow = false) => {
    if (newWindow && capabilities.features.multiWindow) host.window.openThread(path, { newWindow: true });
    else navigate(path);
  }, [capabilities.features.multiWindow, host, navigate]);
  const newThread = useCallback((deviceId?: string, workspaceId?: string, chooseLocation = false) => {
    const id = deviceId || currentDeviceId || index.devices.find(device => device.connected)?.id;
    if (!id || chooseLocation) { setChoosingLocation(true); return; }
    const workspaceParam = workspaceId ?? (!deviceId || deviceId === currentDeviceId ? workspace?.id : undefined);
    navigate(`/devices/${encodeURIComponent(id)}/threads/new${workspaceParam ? `?workspaceId=${encodeURIComponent(workspaceParam)}` : ''}`);
  }, [currentDeviceId, index.devices, navigate, workspace?.id]);
  const revealDevice = useCallback((deviceId: string) => {
    updatePreferences(current => ({ collapsed: false, scope: 'all', provider: '', expanded: { ...current.expanded, [`device:${deviceId}`]: true } }));
    navigate(`/devices/${encodeURIComponent(deviceId)}/workspaces`);
    requestAnimationFrame(() => window.dispatchEvent(new CustomEvent('remote-codex:desktop-reveal-device', { detail: deviceId })));
  }, [navigate, updatePreferences]);
  const toggleInspector = useCallback(() => {
    const click = () => {
      const explorer = content.current?.querySelector<HTMLButtonElement>('[aria-label="Toggle Explorer"]');
      if (explorer) { const opening = explorer.getAttribute('aria-expanded') !== 'true'; explorer.click(); setInspector(opening); return; }
      const workspace = content.current?.querySelector<HTMLButtonElement>('[aria-label="Expand workspace"], [aria-label="Collapse workspace"]');
      if (workspace) { const opening = workspace.getAttribute('aria-label') === 'Expand workspace'; workspace.click(); setInspector(opening); }
    };
    if (!content.current?.querySelector('[aria-label="Toggle Explorer"]')) {
      const tools = content.current?.querySelector<HTMLButtonElement>('[aria-label="Thread tools"]');
      if (tools && tools.getAttribute('aria-expanded') !== 'true') { tools.click(); requestAnimationFrame(click); return; }
    }
    click();
  }, []);
  const travel = useCallback((delta: number) => {
    const next = history.current.cursor + delta;
    if (next < 0 || next >= history.current.paths.length) return;
    history.current.cursor = next; history.current.traversing = true; navigate(history.current.paths[next]!);
  }, [navigate]);
  useEffect(() => {
    const path = location.pathname + location.search;
    if (history.current.traversing) history.current.traversing = false;
    else if (history.current.paths[history.current.cursor] !== path) {
      history.current.paths = [...history.current.paths.slice(0, history.current.cursor + 1), path]; history.current.cursor++;
    }
    setHistoryVersion(current => current + 1);
    setBreadcrumbMenu(null);
    window.dispatchEvent(new Event('remote-codex:desktop-route'));
  }, [location.pathname, location.search]);
  useEffect(() => {
    if (!focusedKey || !focusedWindow) return;
    const timer = setTimeout(() => store.markRead(focusedKey), 1500);
    return () => clearTimeout(timer);
  }, [focusedKey, focusedWindow, store]); // Do not restart the dwell timer on live status changes.
  useEffect(() => {
    const focus = () => setFocusedWindow(document.hasFocus()); window.addEventListener('focus', focus); window.addEventListener('blur', focus);
    return () => { window.removeEventListener('focus', focus); window.removeEventListener('blur', focus); };
  }, []);
  useEffect(() => {
    const root = content.current; if (!root) return;
    const sync = () => setInspector(Boolean(root.querySelector('.matter-explorer')));
    sync();
    const observer = new MutationObserver(sync);
    observer.observe(root, { childList: true, subtree: true });
    return () => observer.disconnect();
  }, [focusedKey]);
  useEffect(() => {
    host.window.setTitle(thread?.title ?? (location.pathname === '/desktop/devices' ? '设备 — Remote Codex' : 'Remote Codex'));
  }, [host, location.pathname, thread?.title]);
  useEffect(() => syncActivity(store, host, () => focusedKeyFromPath(route.current), capabilities.features.badge), [store, host, capabilities.features.badge]);
  useEffect(() => host.onCommand(command => {
    void handleHostCommand(command, { store, navigate, request: deviceRequest, openPalette: () => setPalette(true), newThread: () => newThread() }).then(result => host.commandResult(command.id, result));
  }), [host, navigate, newThread, store]);
  useEffect(() => {
    const root = content.current; if (!root) return;
    const path = location.pathname;
    // ThreadDetailPage consumes global location and owns transport subscriptions.
    // Keeping hidden copies mounted would attach them to the newly selected
    // device. Restore the real timeline's scroll instead of duplicating views.
    const restore = () => {
      const scroller = root.querySelector<HTMLElement>('[data-testid="thread-scroll-container"], .thread-graph-scroll-container') ?? root;
      const saved = scrollPositions.current.get(path);
      if (saved !== undefined) scroller.scrollTop = saved;
    };
    const frame = requestAnimationFrame(restore);
    const observer = new MutationObserver(restore); observer.observe(root, { childList: true, subtree: true });
    const timeout = setTimeout(() => observer.disconnect(), 2000);
    const scroll = (event: Event) => {
      const target = event.target;
      if (!(target instanceof HTMLElement) || !target.matches('[data-testid="thread-scroll-container"], .thread-graph-scroll-container') || target.scrollHeight <= target.clientHeight) return;
      scrollPositions.current.set(path, target.scrollTop);
      if (focusedKey && document.hasFocus() && target.scrollHeight - target.scrollTop - target.clientHeight < 8) store.markRead(focusedKey);
    };
    root.addEventListener('scroll', scroll, true);
    return () => { cancelAnimationFrame(frame); clearTimeout(timeout); observer.disconnect(); root.removeEventListener('scroll', scroll, true); };
  }, [location.pathname, focusedKey, store]);
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      const command = event.metaKey || event.ctrlKey;
      if (command && event.key.toLowerCase() === 'k') { event.preventDefault(); setPalette(current => !current); }
      else if (command && event.key === '\\') { event.preventDefault(); if (event.altKey) toggleInspector(); else updatePreferences(current => ({ collapsed: !current.collapsed })); }
      else if (command && event.key === '[') { event.preventDefault(); travel(-1); }
      else if (command && event.key === ']') { event.preventDefault(); travel(1); }
      else if (command && event.key.toLowerCase() === 'n') { event.preventDefault(); newThread(undefined, undefined, event.shiftKey); }
      else if (command && event.shiftKey && event.key.toLowerCase() === 'f') { event.preventDefault(); updatePreferences({ collapsed: false }); window.dispatchEvent(new Event('remote-codex:desktop-filter')); }
      else if (command && event.shiftKey && event.key.toLowerCase() === 'p' && capabilities.features.pet) { event.preventDefault(); void host.pet.isVisible().then(visible => host.pet.setVisible(!visible)); }
      else if (command && event.key === ',') { event.preventDefault(); shell?.openSettings(); }
      else if (command && /^[1-9]$/.test(event.key)) { event.preventDefault(); const smart = smartThreads(store.getSnapshot(), Date.now()); const list = [...smart.attention, ...smart.running, ...smart.completed].filter((thread, i, all) => all.findIndex(item => item.key === thread.key) === i); const thread = list[Number(event.key) - 1]; if (thread) open(threadPath(thread)); }
      else if (event.altKey && event.shiftKey && ['ArrowDown', 'ArrowUp'].includes(event.key) && !(event.target as HTMLElement).closest('[role="tree"]')) {
        event.preventDefault(); const attention = store.getSnapshot().threads.filter(thread => thread.requests.length || (thread.lastTurnCompletedAt && thread.lastTurnCompletedAt > (thread.lastViewedAt ?? ''))); const current = attention.findIndex(thread => thread.key === focusedKey); const next = attention[(current + (event.key === 'ArrowDown' ? 1 : -1) + attention.length) % attention.length]; if (next) open(threadPath(next));
      }
    };
    window.addEventListener('keydown', keydown); return () => window.removeEventListener('keydown', keydown);
  }, [capabilities.features.pet, focusedKey, host, newThread, open, shell, store, toggleInspector, travel, updatePreferences]);
  const actions: PaletteAction[] = useMemo(() => [
    { id: 'new', label: '新线程', shortcut: '⌘N', run: () => newThread() },
    { id: 'switch-device', label: '切换设备', run: () => { navigate('/desktop/devices'); } },
    { id: 'devices', label: '打开设备总览', run: () => navigate('/desktop/devices') },
    ...(capabilities.features.pet ? [{ id: 'pet', label: '切换宠物显示', shortcut: '⌘⇧P', run: () => { void host.pet.isVisible().then(visible => host.pet.setVisible(!visible)); } }] : []),
    { id: 'snooze', label: '暂停通知 1 小时', run: () => { void host.settings.set({ snoozeUntil: new Date(Date.now() + 3600_000).toISOString() }); } },
    { id: 'theme', label: '切换亮色 / 暗色主题', run: () => shell?.setThemeMode(shell.effectiveTheme === 'dark' ? 'light' : 'dark') },
    { id: 'browser', label: '在浏览器打开当前页', run: () => host.shell.openExternal(location.pathname.startsWith('/desktop') ? `${window.location.origin}/relay-devices` : window.location.href) },
    ...buildDeviceActions(index, { navigate, onRevealDevice: revealDevice, onNewThread: newThread }),
  ], [capabilities.features.pet, host, index, location.pathname, navigate, newThread, revealDevice, shell]);
  const breadcrumbs = [
    { key: 'device', label: device?.name ?? '所有设备', items: index.devices.map(device => ({ label: device.name, path: `/devices/${encodeURIComponent(device.id)}/workspaces` })) },
    ...(workspace ? [{ key: 'workspace', label: workspace.label, items: index.workspaces.filter(item => item.deviceId === currentDeviceId).map(item => ({ label: item.label, path: `/devices/${encodeURIComponent(item.deviceId)}/threads?workspaceId=${encodeURIComponent(item.id)}` })) }] : []),
    ...(thread ? [{ key: 'thread', label: thread.title, items: index.threads.filter(item => item.deviceId === currentDeviceId && item.workspaceId === thread.workspaceId && !item.closedAt).map(item => ({ label: item.title, path: threadPath(item) })) }] : []),
  ];
  const chrome = capabilities.windowChrome;
  const style = { '--rc-sidebar-width': `${preferences.width}px`, '--rc-chrome-left': chrome.controlsSide === 'left' ? `${chrome.trafficLightsWidth}px` : '8px', '--rc-chrome-right': chrome.controlsSide === 'right' ? `${chrome.trafficLightsWidth}px` : '8px' } as CSSProperties;
  return <div className="rc-desktop" style={style} data-desktop-surface data-history-version={historyVersion}>
    <header className="rc-toolbar">
      <button className="rc-toolbar-button" aria-label="切换侧栏" title="切换侧栏 ⌘\\" onClick={() => updatePreferences(current => ({ collapsed: !current.collapsed }))}><PanelLeft size={16} /></button>
      <div className="rc-history"><button aria-label="后退" disabled={history.current.cursor === 0} onClick={() => travel(-1)}><ChevronLeft size={16} /></button><button aria-label="前进" disabled={history.current.cursor >= history.current.paths.length - 1} onClick={() => travel(1)}><ChevronRight size={16} /></button></div>
      <nav className="rc-breadcrumb" aria-label="面包屑">{breadcrumbs.map((segment, i) => <div key={segment.key}>{i ? <ChevronRight size={11} /> : null}<button aria-expanded={breadcrumbMenu === segment.key} onClick={() => setBreadcrumbMenu(current => current === segment.key ? null : segment.key)}><span>{segment.label}</span><ChevronDown size={10} /></button>{breadcrumbMenu === segment.key ? <><div className="rc-breadcrumb-dismiss" onClick={() => setBreadcrumbMenu(null)} /><div className="rc-breadcrumb-menu" role="menu" onKeyDown={event => menuKeyboard(event, () => setBreadcrumbMenu(null))}>{segment.items.map((item, i) => <button key={item.path} role="menuitem" autoFocus={i === 0} onClick={() => { open(item.path); setBreadcrumbMenu(null); }}>{item.label}</button>)}</div></> : null}</div>)}</nav>
      <button className="rc-toolbar-search" aria-label="打开命令面板" onClick={() => setPalette(true)}><Search size={13} /><span>搜索线程、工作区、命令</span><kbd>⌘K</kbd></button>
      <button className="rc-toolbar-button" aria-label="新建线程" title="新线程 ⌘N" onClick={() => newThread()}><Plus size={17} /></button>
      <button className={`rc-toolbar-button ${inspector ? 'is-active' : ''}`} aria-label="切换检查器" title="检查器 ⌘⌥\\" onClick={toggleInspector} disabled={!focusedKey}><PanelRight size={16} /></button>
    </header>
    <div className="rc-body">
      {!preferences.collapsed ? <aside className="rc-sidebar" aria-label="桌面侧栏"><DesktopTree preferences={preferences} updatePreferences={updatePreferences} selectedKey={focusedKey} selectedDeviceId={focusedKey ? null : currentDeviceId || null} onOpen={open} onNewThread={newThread} /><footer className="rc-sidebar-footer"><button aria-label="设备总览" onClick={() => navigate('/desktop/devices')}><Monitor size={14} /><span>设备</span><span className="rc-footer-count">{index.devices.length}</span></button><div><button aria-label="账号" title="账号" onClick={() => navigate('/relay-account')}><span className="rc-avatar">{index.accountId.slice(0, 1).toUpperCase()}</span></button><button aria-label="设置" title="设置 ⌘," onClick={() => shell?.openSettings()}><Settings size={14} /></button>{capabilities.features.pet ? <button aria-label="切换宠物" title="切换宠物 ⌘⇧P" onClick={() => { void host.pet.isVisible().then(visible => host.pet.setVisible(!visible)); }}><Sparkles size={14} /></button> : null}<button aria-label="帮助" title="帮助" onClick={() => host.shell.openExternal(`${window.location.origin}/relay-guide`)}><CircleHelp size={14} /></button></div></footer><div className="rc-sidebar-resizer" role="separator" aria-orientation="vertical" aria-label="侧栏宽度" aria-valuemin={180} aria-valuemax={420} aria-valuenow={preferences.width} tabIndex={0} onKeyDown={event => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); updatePreferences({ width: Math.max(180, Math.min(420, preferences.width + (event.key === 'ArrowLeft' ? -10 : 10))) }); } }} onPointerDown={event => {
        event.currentTarget.setPointerCapture(event.pointerId); const start = event.clientX; const width = preferences.width;
        const move = (move: PointerEvent) => updatePreferences({ width: Math.max(180, Math.min(420, width + move.clientX - start)) });
        const end = () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', end); document.body.style.cursor = ''; document.body.style.userSelect = ''; };
        document.body.style.cursor = 'col-resize'; document.body.style.userSelect = 'none'; window.addEventListener('pointermove', move); window.addEventListener('pointerup', end, { once: true });
      }} /></aside> : null}
      <main ref={content} className={`rc-main ${focusedKey ? 'rc-main-thread' : ''}`}>
        {['/', '/desktop', '/relay-devices'].includes(location.pathname) ? <DesktopHome onOpen={open} onNew={() => newThread()} /> : location.pathname === '/desktop/devices' ? <Suspense fallback={null}><DevicesPage onRevealDevice={revealDevice} onNewThread={newThread} /></Suspense> : children}
      </main>
    </div>
    {choosingLocation ? <LocationPicker onClose={() => setChoosingLocation(false)} onChoose={(deviceId, workspaceId) => { setChoosingLocation(false); newThread(deviceId, workspaceId); }} /> : null}
    {palette ? <CommandPalette onClose={() => setPalette(false)} onNavigate={open} actions={actions} /> : null}
  </div>;
}
