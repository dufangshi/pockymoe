import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { ArrowUpRight, Command, CornerDownLeft, Folder, Monitor, Search, Sparkles } from 'lucide-react';
import { useDesktopSelector, useDesktopStore } from '../data';
import { needsAttention, threadPath } from '../data/types';
import { fuzzyMatch } from './model';
import { Highlight } from './Tree';
import { StatusGlyph } from './StatusGlyph';

export interface PaletteAction { id: string; label: string; shortcut?: string; run(): void }
interface Result { id: string; group: string; label: string; path: string; icon: ReactNode; shortcut?: string; run(): void }
export function CommandPalette({ onClose, onNavigate, actions }: { onClose(): void; onNavigate(path: string): void; actions: readonly PaletteAction[] }) {
  const index = useDesktopSelector(index => index);
  const store = useDesktopStore();
  const [query, setQuery] = useState(''); const [selected, setSelected] = useState(0);
  const input = useRef<HTMLInputElement>(null); const panel = useRef<HTMLDivElement>(null);
  const results = useMemo(() => {
    const list: Result[] = [];
    const device = (id: string) => index.devices.find(device => device.id === id)?.name ?? '';
    const workspace = (deviceId: string, id: string) => index.workspaces.find(workspace => workspace.deviceId === deviceId && workspace.id === id)?.label ?? '';
    const addThread = (thread: typeof index.threads[number], group: string) => list.push({ id: thread.key, group, label: thread.title, path: `${device(thread.deviceId)} › ${workspace(thread.deviceId, thread.workspaceId)}`, icon: <StatusGlyph thread={thread} />, run: () => onNavigate(threadPath(thread)) });
    if (query.trim()) {
      index.threads.filter(thread => fuzzyMatch(`${thread.title} ${thread.agentName ?? ''} ${workspace(thread.deviceId, thread.workspaceId)} ${device(thread.deviceId)}`, query) !== null).sort((a, b) => Number(b.isPinned) - Number(a.isPinned) || b.updatedAt.localeCompare(a.updatedAt)).slice(0, 50).forEach(thread => addThread(thread, '线程'));
      index.workspaces.filter(workspace => fuzzyMatch(workspace.label, query) !== null).slice(0, 15).forEach(workspace => list.push({ id: `w:${workspace.key}`, group: '工作区', label: workspace.label, path: device(workspace.deviceId), icon: <Folder size={15} />, run: () => onNavigate(`/devices/${encodeURIComponent(workspace.deviceId)}/threads?workspaceId=${encodeURIComponent(workspace.id)}`) }));
      index.devices.filter(device => fuzzyMatch(device.name, query) !== null).forEach(device => list.push({ id: `d:${device.id}`, group: '设备', label: device.name, path: device.connected ? '在线' : '离线', icon: <Monitor size={15} />, run: () => onNavigate(`/devices/${encodeURIComponent(device.id)}/workspaces`) }));
    } else {
      const attention = index.threads.filter(thread => needsAttention(thread));
      attention.slice(0, 8).forEach(thread => addThread(thread, '需要你'));
      [...index.threads].filter(thread => thread.lastViewedAt && !attention.includes(thread)).sort((a, b) => (b.lastViewedAt ?? '').localeCompare(a.lastViewedAt ?? '')).slice(0, 8).forEach(thread => addThread(thread, '最近打开'));
    }
    actions.filter(action => fuzzyMatch(action.label, query) !== null).forEach(action => list.push({ ...action, group: query ? '操作' : '常用操作', path: '', icon: <Command size={14} /> }));
    return list;
  }, [actions, index, onNavigate, query]);
  useEffect(() => { setSelected(0); }, [query]);
  useEffect(() => { const previous = document.activeElement as HTMLElement | null; input.current?.focus(); return () => previous?.focus(); }, []);
  useEffect(() => { panel.current?.querySelector(`[data-palette-index="${selected}"]`)?.scrollIntoView({ block: 'nearest' }); }, [selected]);
  const choose = (result: Result | undefined) => { if (result) { result.run(); onClose(); } };
  let lastGroup = '';
  return <div className="rc-palette-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}>
    <div className="rc-palette" ref={panel} role="dialog" aria-modal="true" aria-label="命令面板" onKeyDown={event => {
      if (event.key === 'Escape') { event.preventDefault(); onClose(); }
      else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); setSelected(current => (current + (event.key === 'ArrowDown' ? 1 : -1) + results.length) % Math.max(1, results.length)); }
      else if (event.key === 'Enter') { event.preventDefault(); choose(results[selected]); }
      else if (event.key === 'Tab') { event.preventDefault(); input.current?.focus(); }
    }}>
      <div className="rc-palette-input"><Search size={18} /><input ref={input} role="combobox" aria-expanded="true" aria-controls="rc-palette-results" aria-activedescendant={results[selected] ? `rc-palette-option-${selected}` : undefined} aria-label="搜索线程、工作区、命令" placeholder="搜索线程、工作区、命令…" value={query} onChange={event => setQuery(event.target.value)} /><kbd>esc</kbd></div>
      <div className="rc-palette-results" id="rc-palette-results" role="listbox" aria-label="搜索结果">
        {results.map((result, i) => { const heading = result.group !== lastGroup; lastGroup = result.group; return <div key={result.id}>{heading ? <div className="rc-palette-group" role="presentation">{result.group}</div> : null}<div id={`rc-palette-option-${i}`} role="option" aria-selected={selected === i} data-palette-index={i} className={`rc-palette-option ${selected === i ? 'is-selected' : ''}`} onMouseEnter={() => { setSelected(i); const thread = index.threads.find(thread => thread.key === result.id); if (thread) store.prefetchDetail(thread.deviceId, thread.id); }} onClick={() => choose(result)}>{result.icon}<span className="rc-palette-label"><Highlight label={result.label} ranges={fuzzyMatch(result.label, query) ?? []} /></span><span className="rc-palette-path">{result.path}</span>{result.shortcut ? <kbd>{result.shortcut}</kbd> : selected === i ? <CornerDownLeft size={13} /> : <ArrowUpRight size={12} className="rc-palette-arrow" />}</div></div>; })}
        {!results.length ? <div className="rc-palette-empty"><Sparkles size={24} /><p>没有找到“{query}”</p><span>试试线程标题、工作区或设备名称</span></div> : null}
      </div>
      <div className="rc-palette-footer"><span><kbd>↑</kbd><kbd>↓</kbd> 选择 <kbd>↵</kbd> 打开</span><span>所有设备 · 本地索引</span></div>
    </div>
  </div>;
}
