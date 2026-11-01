import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { ChevronDown, ChevronRight, Ellipsis, Folder, Monitor, Pencil, Pin, Plus, RefreshCw, Search, X } from 'lucide-react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { deviceRequest } from '../../lib/api';
import type { ThreadDto } from '@remote-codex/shared';
import { useDesktopSelector, useDesktopStore } from '../data';
import { active, needsAttention, running, threadPath, unread, type DesktopThread } from '../data/types';
import { mergeRows } from '../data/store';
import { buildTree, freezeRows, type HighlightRange, type TreeRow } from './model';
import { StatusGlyph } from './StatusGlyph';
import { menuKeyboard } from './keyboard';
import type { NavPreferences } from './preferences';

export function Highlight({ label, ranges }: { label: string; ranges: readonly HighlightRange[] }) {
  let cursor = 0; const parts = [];
  for (const [start, end] of ranges) { parts.push(label.slice(cursor, start), <mark key={start}>{label.slice(start, end)}</mark>); cursor = end; }
  parts.push(label.slice(cursor)); return <>{parts}</>;
}
function relativeTime(value: string, now = Date.now()) {
  const minutes = Math.max(0, Math.floor((now - Date.parse(value)) / 60000));
  return minutes < 1 ? '刚刚' : minutes < 60 ? `${minutes} 分钟` : minutes < 1440 ? `${Math.floor(minutes / 60)} 小时` : minutes < 2880 ? '昨天' : `${Math.floor(minutes / 1440)} 天`;
}
function Timer({ since }: { since: string | null }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(timer); }, []);
  const elapsed = since ? Math.max(0, Math.floor((now - Date.parse(since)) / 1000)) : 0;
  return <span className="rc-timer">{String(Math.floor(elapsed / 60)).padStart(2, '0')}:{String(elapsed % 60).padStart(2, '0')}</span>;
}
function ThreadMeta({ thread }: { thread: DesktopThread }) {
  return thread.requests.length ? <span className="rc-request-label">{thread.requests[0]?.kind === 'input' ? '输入' : '审批'}</span> : running(thread) ? <Timer since={thread.lastTurnStartedAt} /> : <span>{relativeTime(thread.updatedAt)}</span>;
}
const NavRow = memo(function NavRow({ row, selected, focused, onOpen, onToggle, onAction, onHover }: {
  row: TreeRow; selected: boolean; focused: boolean;
  onOpen(row: TreeRow, newWindow?: boolean): void; onToggle(row: TreeRow): void;
  onAction(row: TreeRow, action: string): void; onHover(row: TreeRow | null): void;
}) {
  const thread = useDesktopSelector(index => row.threadKey ? index.threads.find(thread => thread.key === row.threadKey) : undefined);
  const defaultOpen = () => row.kind === 'thread' ? onOpen(row) : onToggle(row);
  return <div role="treeitem" id={`rc-row-${row.id}`} aria-level={row.depth + 1} aria-selected={selected} {...(row.expandable ? { 'aria-expanded': row.expanded } : {})}
    data-row-id={row.id} data-thread-key={row.threadKey} data-kind={row.kind}
    className={`rc-tree-row ${selected ? 'is-selected' : ''} ${focused ? 'is-focused' : ''} ${row.offline || thread?.closedAt ? 'is-muted' : ''} ${row.kind === 'section' ? 'rc-smart-row' : ''} ${row.exiting ? 'rc-row-exiting' : ''}`}
    style={{ paddingLeft: 8 + row.depth * 12 }} onClick={defaultOpen} onDoubleClick={() => { if (row.kind !== 'thread') onOpen(row); }} onMouseEnter={() => onHover(row)} onMouseLeave={() => onHover(null)} title={row.label}>
    <button className="rc-disclosure" tabIndex={-1} aria-label={row.expanded ? `折叠 ${row.label}` : `展开 ${row.label}`} onClick={event => { event.stopPropagation(); if (row.expandable) onToggle(row); }}>
      {row.expandable ? row.expanded ? <ChevronDown size={12} /> : <ChevronRight size={12} /> : null}
    </button>
    {thread ? <StatusGlyph thread={thread} /> : row.kind === 'device' ? <Monitor size={14} className="rc-node-icon" /> : row.kind === 'workspace' ? <Folder size={14} className="rc-node-icon" /> : <span className={`rc-section-dot rc-section-${row.id.split(':')[1]}`} />}
    <span className="rc-row-label"><Highlight label={row.label} ranges={row.highlights} /></span>
    {thread?.isPinned ? <Pin size={10} className="rc-pin" /> : null}
    <span className="rc-row-meta">
      {thread ? <ThreadMeta thread={thread} /> : row.kind === 'section' ? <span className="rc-count">{row.count || '—'}</span> : row.offline ? <span>离线{row.lastSeenAt ? ` · ${relativeTime(row.lastSeenAt)}${Date.now() - Date.parse(row.lastSeenAt) < 86400000 ? '前' : ''}` : ''}</span> : !row.expanded && (row.rollup.running || row.rollup.attention || row.rollup.unread) ? <span className="rc-rollup">{row.rollup.running ? <span className="rc-running-text">◐ {row.rollup.running}</span> : null}{row.rollup.attention ? <span className="rc-attention-text">● {row.rollup.attention}</span> : row.rollup.unread ? <span>● {row.rollup.unread}</span> : null}</span> : row.kind === 'device' ? <span className="rc-online-dot" /> : null}
    </span>
    {['thread', 'workspace', 'device'].includes(row.kind) ? <span className="rc-row-actions">
      <button tabIndex={-1} title={thread ? '置顶线程' : row.kind === 'workspace' ? '新线程' : '刷新设备'} aria-label={thread ? '置顶线程' : row.kind === 'workspace' ? '新线程' : '刷新设备'} onClick={event => { event.stopPropagation(); onAction(row, thread ? 'pin' : row.kind === 'workspace' ? 'new' : 'refresh'); }}>{thread ? <Pin size={12} /> : row.kind === 'workspace' ? <Plus size={13} /> : <RefreshCw size={12} />}</button>
      {thread ? <button tabIndex={-1} title="重命名线程" aria-label="重命名线程" onClick={event => { event.stopPropagation(); onAction(row, 'rename'); }}><Pencil size={12} /></button> : null}
      <button tabIndex={-1} title="更多操作" aria-label="更多操作" onClick={event => { event.stopPropagation(); onAction(row, 'more'); }}><Ellipsis size={14} /></button>
    </span> : null}
  </div>;
});

export function DesktopTree({ preferences, updatePreferences, selectedKey, onOpen, onNewThread }: {
  preferences: NavPreferences;
  updatePreferences(patch: Partial<NavPreferences> | ((current: NavPreferences) => Partial<NavPreferences>)): void;
  selectedKey: string | null;
  onOpen(path: string, newWindow?: boolean): void;
  onNewThread(deviceId?: string, workspaceId?: string): void;
}) {
  const store = useDesktopStore();
  const index = useDesktopSelector(index => index);
  const [query, setQuery] = useState('');
  const [filterOpen, setFilterOpen] = useState(false);
  const [revealId, setRevealId] = useState<string | null>(null);
  const [focusedId, setFocusedId] = useState<string | null>(null);
  const [frozen, setFrozen] = useState(false);
  const [menu, setMenu] = useState<TreeRow | null>(null);
  const [rename, setRename] = useState<{ row: TreeRow; title: string } | null>(null);
  const [error, setError] = useState('');
  const [showLoading, setShowLoading] = useState(false);
  const container = useRef<HTMLDivElement>(null);
  const filter = useRef<HTMLInputElement>(null);
  const pointer = useRef(false); const treeFocused = useRef(false);
  const releaseTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const hoverTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const previousRows = useRef<readonly TreeRow[]>([]);
  const exitingRows = useRef(new Map<string, { row: TreeRow; expiresAt: number; index: number }>());
  const [animationTick, setAnimationTick] = useState(0);
  const painted = useRef(false);
  const positions = useRef(new Map<string, number>());
  const structure = `${query}:${preferences.scope}:${preferences.provider}:${JSON.stringify(preferences.expanded)}`;
  const previousStructure = useRef(structure);
  const desired = useMemo(() => buildTree(index, { query, scope: preferences.scope, provider: preferences.provider, expanded: preferences.expanded, now: Date.now() }), [index, query, preferences.scope, preferences.provider, preferences.expanded]);
  const rows = useMemo(() => {
    const engaged = frozen && previousStructure.current === structure;
    const now = Math.max(Date.now(), animationTick);
    const next = [...freezeRows(previousRows.current, desired, engaged)].filter(row => !row.exiting || (exitingRows.current.get(row.id)?.expiresAt ?? 0) > now);
    for (const [id, entry] of exitingRows.current) if (entry.expiresAt <= now) exitingRows.current.delete(id);
    if (!engaged) {
      const ids = new Set(next.map(row => row.id));
      previousRows.current.forEach((row, index) => {
        if (!ids.has(row.id) && !row.exiting && !exitingRows.current.has(row.id)) exitingRows.current.set(row.id, { row: { ...row, exiting: true }, expiresAt: now + 120, index });
      });
      for (const [id, entry] of exitingRows.current) {
        if (ids.has(id) || entry.expiresAt <= now) exitingRows.current.delete(id);
        else next.splice(Math.min(entry.index, next.length), 0, entry.row);
      }
    }
    const stable = mergeRows(previousRows.current, next, row => row.id);
    previousStructure.current = structure; previousRows.current = stable; return stable;
  }, [desired, frozen, structure, animationTick]);
  useEffect(() => {
    if (!rows.some(row => row.exiting)) return;
    const timer = setTimeout(() => setAnimationTick(Date.now()), 120);
    return () => clearTimeout(timer);
  }, [rows]);
  const virtualizer = useVirtualizer({ count: rows.length, getScrollElement: () => container.current, estimateSize: () => 26, overscan: 12, getItemKey: index => rows[index]!.id });
  useLayoutEffect(() => {
    if (!container.current) return;
    if (!painted.current && rows.some(row => row.kind === 'device') && performance.getEntriesByName('rc-desktop-shell-mount').length) {
      painted.current = true; performance.measure('rc-desktop-tree-paint', 'rc-desktop-shell-mount');
    }
    const next = new Map<string, number>();
    for (const element of Array.from(container.current.querySelectorAll<HTMLElement>('[data-row-id]'))) {
      const id = element.dataset.rowId!; const top = element.getBoundingClientRect().top; next.set(id, top);
      const old = positions.current.get(id);
      if (!frozen && old !== undefined && old !== top && !matchMedia('(prefers-reduced-motion: reduce)').matches) element.animate?.([{ transform: `translateY(${old - top}px)` }, { transform: 'translateY(0)' }], { duration: 160, easing: 'cubic-bezier(.2,.8,.2,1)' });
    }
    positions.current = next;
  }, [rows, frozen]);
  useEffect(() => {
    const timer = setTimeout(() => setShowLoading(true), 200);
    return () => { clearTimeout(timer); clearTimeout(releaseTimer.current); clearTimeout(hoverTimer.current); };
  }, []);
  useEffect(() => {
    const listener = () => { setFilterOpen(true); virtualizer.scrollToIndex(rows.findIndex(row => row.kind === 'filter'), { align: 'auto' }); requestAnimationFrame(() => filter.current?.focus()); };
    window.addEventListener('remote-codex:desktop-filter', listener); return () => window.removeEventListener('remote-codex:desktop-filter', listener);
  }, [rows, virtualizer]);
  useEffect(() => {
    const reveal = (event: Event) => { setQuery(''); setRevealId(`device:${(event as CustomEvent<string>).detail}`); };
    window.addEventListener('remote-codex:desktop-reveal-device', reveal);
    return () => window.removeEventListener('remote-codex:desktop-reveal-device', reveal);
  }, []);
  useEffect(() => {
    if (!revealId) return;
    const index = rows.findIndex(row => row.id === revealId);
    if (index < 0) return;
    virtualizer.scrollToIndex(index, { align: 'start' }); setFocusedId(revealId); setRevealId(null); container.current?.focus();
  }, [revealId, rows, virtualizer]);
  const toggle = useCallback((row: TreeRow) => updatePreferences(current => ({ expanded: { ...current.expanded, [row.id]: !row.expanded } })), [updatePreferences]);
  const open = useCallback((row: TreeRow, newWindow = false) => {
    if (row.threadKey) { const thread = store.getSnapshot().threads.find(thread => thread.key === row.threadKey); if (thread) onOpen(threadPath(thread), newWindow); }
    else if (row.kind === 'device') onOpen(`/devices/${encodeURIComponent(row.deviceId!)}/workspaces`);
    else if (row.kind === 'workspace') onOpen(`/devices/${encodeURIComponent(row.deviceId!)}/threads?workspaceId=${encodeURIComponent(row.workspaceId!)}`);
    else updatePreferences(current => ({ expanded: { ...current.expanded, [row.id]: !row.expanded } }));
  }, [onOpen, store, updatePreferences]);
  const action = useCallback((row: TreeRow, action: string) => {
    if (action === 'more') { setMenu(row); return; }
    if (action === 'rename') { setRename({ row, title: row.label }); return; }
    if (action === 'new') { onNewThread(row.deviceId, row.workspaceId); return; }
    if (action === 'refresh') { void store.refreshDevice(row.deviceId!); return; }
    const thread = store.getSnapshot().threads.find(thread => thread.key === row.threadKey);
    if (action === 'pin' && thread) void deviceRequest<ThreadDto>(thread.deviceId, `/api/threads/${encodeURIComponent(thread.id)}`, { method: 'PATCH', body: JSON.stringify({ isPinned: !thread.isPinned }) }).then(updated => store.patchThread(thread.deviceId, updated)).catch(error => setError(String(error)));
  }, [onNewThread, store]);
  const hover = useCallback((row: TreeRow | null) => {
    clearTimeout(hoverTimer.current);
    if (row?.threadKey) { const thread = store.getSnapshot().threads.find(thread => thread.key === row.threadKey); if (thread) hoverTimer.current = setTimeout(() => store.prefetchDetail(thread.deviceId, thread.id), 120); }
  }, [store]);
  const release = () => { clearTimeout(releaseTimer.current); releaseTimer.current = setTimeout(() => { if (!pointer.current && !treeFocused.current) setFrozen(false); }, 800); };
  const keyboardRows = rows.filter(row => !row.exiting && row.kind !== 'filter' && row.kind !== 'scope');
  const focusRow = (index: number) => { const row = keyboardRows[index]; if (!row) return; setFocusedId(row.id); virtualizer.scrollToIndex(rows.findIndex(item => item.id === row.id), { align: 'auto' }); };
  const keyDown = (event: KeyboardEvent) => {
    if (event.target instanceof HTMLInputElement || event.target instanceof HTMLSelectElement) return;
    const index = Math.max(0, keyboardRows.findIndex(row => row.id === focusedId)); const row = keyboardRows[index]; if (!row) return;
    if (event.altKey && event.shiftKey && ['ArrowDown', 'ArrowUp'].includes(event.key)) {
      event.preventDefault();
      const direction = event.key === 'ArrowDown' ? 1 : -1;
      for (let offset = 1; offset <= keyboardRows.length; offset++) { const next = (index + offset * direction + keyboardRows.length) % keyboardRows.length; const candidate = keyboardRows[next]; const thread = store.getSnapshot().threads.find(thread => thread.key === candidate?.threadKey); if (thread && (needsAttention(thread) || unread(thread))) { focusRow(next); open(candidate!); return; } }
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); focusRow(Math.max(0, Math.min(keyboardRows.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)))); }
    else if (event.key === 'Home' || event.key === 'End') { event.preventDefault(); focusRow(event.key === 'Home' ? 0 : keyboardRows.length - 1); }
    else if (event.key === 'ArrowRight') { event.preventDefault(); if (row.expandable && !row.expanded) toggle(row); else focusRow(index + 1); }
    else if (event.key === 'ArrowLeft') { event.preventDefault(); if (row.expandable && row.expanded) toggle(row); else if (row.parentId) focusRow(keyboardRows.findIndex(candidate => candidate.id === row.parentId)); }
    else if (event.key === 'Enter') { event.preventDefault(); open(row, event.metaKey || event.ctrlKey); }
    else if (event.key === 'F2' && row.threadKey) { event.preventDefault(); setRename({ row, title: row.label }); }
    else if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) { event.preventDefault(); setMenu(row); }
  };
  return <div className="rc-navigation" onPointerEnter={() => { pointer.current = true; clearTimeout(releaseTimer.current); setFrozen(true); }} onPointerLeave={() => { pointer.current = false; release(); }} data-reorder-frozen={frozen}>
    <div className="rc-sidebar-heading"><span>工作空间</span><button title="新线程" aria-label="新线程" onClick={() => onNewThread()}><Plus size={14} /></button></div>
    <div ref={container} className="rc-tree-scroll" role="tree" aria-label="跨设备线程" aria-activedescendant={focusedId ? `rc-row-${focusedId}` : undefined} tabIndex={0} onKeyDown={keyDown} onFocus={() => { treeFocused.current = true; setFrozen(true); if (!focusedId) setFocusedId(rows[0]?.id ?? null); }} onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) { treeFocused.current = false; release(); } }}>
      {index.devices.length === 0 ? index.refreshing && showLoading ? <div className="rc-tree-skeleton" data-testid="desktop-skeleton">{Array.from({ length: 6 }, (_, i) => <span key={i} />)}</div> : showLoading && !index.refreshing ? <div className="rc-tree-empty">{index.error ? '暂时无法连接。缓存会在这里保留。' : '连接设备后，你的工作会显示在这里。'}<button onClick={() => { void store.refresh(); }}>刷新</button></div> : null : <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
        {virtualizer.getVirtualItems().map(item => { const row = rows[item.index]!; return <div key={row.id} style={{ position: 'absolute', top: item.start, left: 0, right: 0, height: 26 }}>{row.kind === 'filter' ? <div className={`rc-tree-filter ${filterOpen || query ? 'is-open' : ''}`}>
      <button aria-label="过滤线程" title="过滤线程 ⌘⇧F" onClick={() => { setFilterOpen(true); requestAnimationFrame(() => filter.current?.focus()); }}><Search size={14} /></button>
      <input ref={filter} aria-label="过滤线程" placeholder="过滤线程…" value={query} onChange={event => setQuery(event.target.value)} onBlur={() => { if (!query) setFilterOpen(false); }} onKeyDown={event => { if (event.key === 'Escape') { setQuery(''); filter.current?.blur(); } if (event.key === 'ArrowDown') { container.current?.focus(); focusRow(0); } }} />
      {query ? <button aria-label="清除过滤" onClick={() => setQuery('')}><X size={12} /></button> : null}
      <select aria-label="提供方" value={preferences.provider} onChange={event => updatePreferences({ provider: event.target.value })}><option value="">所有提供方</option>{['codex', 'claude', 'acp', 'opencode'].map(provider => <option key={provider} value={provider}>{provider === 'acp' ? 'ACP' : provider[0]!.toUpperCase() + provider.slice(1)}</option>)}</select>
    </div> : row.kind === 'scope' ? <div className="rc-scope" aria-label="线程范围">{([['all', '全部'], ['active', '活跃'], ['pinned', '置顶']] as const).map(([scope, label]) => <button key={scope} aria-pressed={preferences.scope === scope} onClick={() => updatePreferences({ scope })}>{label}{scope === 'active' ? <span>{index.threads.filter(thread => active(thread)).length}</span> : null}</button>)}</div> : <NavRow row={row} selected={Boolean(row.threadKey && row.threadKey === selectedKey)} focused={focusedId === row.id} onOpen={open} onToggle={toggle} onAction={action} onHover={hover} />}</div>; })}
      </div>}
    </div>
    {error ? <div role="alert" className="rc-inline-error">{error}<button aria-label="关闭错误" onClick={() => setError('')}>×</button></div> : null}
    {menu ? <div className="rc-modal-backdrop" onMouseDown={() => setMenu(null)}><div className="rc-context-menu" role="menu" aria-label={`${menu.label} 的操作`} onKeyDown={event => menuKeyboard(event, () => setMenu(null))} onMouseDown={event => event.stopPropagation()}>
      <p>{menu.label}</p><button role="menuitem" autoFocus onClick={() => { open(menu, true); setMenu(null); }}>在新窗口打开</button>
      {menu.threadKey ? <><button role="menuitem" onClick={() => { setRename({ row: menu, title: menu.label }); setMenu(null); }}>重命名…</button><button role="menuitem" onClick={() => { store.markRead(menu.threadKey!); setMenu(null); }}>标记已读</button><button role="menuitem" onClick={() => { action(menu, 'pin'); setMenu(null); }}>切换置顶</button></> : <button role="menuitem" onClick={() => { onNewThread(menu.deviceId, menu.workspaceId); setMenu(null); }}>新线程</button>}
      <button role="menuitem" onClick={() => setMenu(null)}>取消</button></div></div> : null}
    {rename ? <div className="rc-modal-backdrop"><form className="rc-rename-dialog" role="dialog" aria-label="重命名线程" onSubmit={event => { event.preventDefault(); const thread = store.getSnapshot().threads.find(thread => thread.key === rename.row.threadKey); if (!thread) return; void deviceRequest<ThreadDto>(thread.deviceId, `/api/threads/${encodeURIComponent(thread.id)}`, { method: 'PATCH', body: JSON.stringify({ title: rename.title.trim() }) }).then(updated => { store.patchThread(thread.deviceId, updated); setRename(null); }).catch(error => setError(String(error))); }}><h2>重命名线程</h2><input autoFocus aria-label="线程标题" value={rename.title} onChange={event => setRename({ ...rename, title: event.target.value })} onKeyDown={event => { if (event.key === 'Escape') setRename(null); }} /><div><button type="button" onClick={() => setRename(null)}>取消</button><button type="submit" disabled={!rename.title.trim()}>保存</button></div></form></div> : null}
  </div>;
}
