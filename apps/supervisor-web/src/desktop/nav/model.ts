import { active, failed, needsAttention, running, unread, type DesktopIndex, type DesktopThread } from '../data/types';

export type Scope = 'all' | 'active' | 'pinned';
export type HighlightRange = readonly [number, number];
export interface TreeOptions {
  query: string;
  scope: Scope;
  provider: string;
  expanded: Readonly<Record<string, boolean>>;
  now: number;
}
export interface Rollup { running: number; attention: number; unread: number }
export interface TreeRow {
  id: string;
  parentId: string | null;
  kind: 'section' | 'device' | 'workspace' | 'thread' | 'more' | 'closed' | 'inactive' | 'filter' | 'scope';
  label: string;
  depth: number;
  deviceId?: string;
  workspaceId?: string;
  threadKey?: string;
  expandable: boolean;
  expanded: boolean;
  rollup: Rollup;
  highlights: readonly HighlightRange[];
  offline?: boolean;
  lastSeenAt?: string | null;
  exiting?: boolean;
  count?: number;
}
export const emptyRollup = (): Rollup => ({ running: 0, attention: 0, unread: 0 });
export function fuzzyMatch(text: string, query: string): HighlightRange[] | null {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return [];
  const haystack = text.toLocaleLowerCase();
  const positions: number[] = []; let cursor = 0;
  for (const char of needle) {
    if (/\s/.test(char)) continue;
    const index = haystack.indexOf(char, cursor);
    if (index === -1) return null;
    positions.push(index); cursor = index + char.length;
  }
  const ranges: [number, number][] = [];
  for (const index of positions) {
    const last = ranges.at(-1);
    if (last && last[1] === index) last[1]++;
    else ranges.push([index, index + 1]);
  }
  return ranges;
}
export function rollup(threads: readonly DesktopThread[], now: number): Rollup {
  return threads.reduce((sum, thread) => ({
    running: sum.running + Number(running(thread)), attention: sum.attention + Number(needsAttention(thread, now)),
    unread: sum.unread + Number(unread(thread)),
  }), emptyRollup());
}
export const compareThreads = (a: DesktopThread, b: DesktopThread) => Number(b.isPinned) - Number(a.isPinned) || b.updatedAt.localeCompare(a.updatedAt) || a.key.localeCompare(b.key);
export function smartThreads(index: DesktopIndex, now: number) {
  return {
    attention: index.threads.filter(thread => needsAttention(thread, now)).sort(compareThreads),
    running: index.threads.filter(running).sort(compareThreads),
    completed: index.threads.filter(thread => unread(thread) && !running(thread) && !failed(thread) && !thread.requests.length).sort(compareThreads),
  };
}
export function buildTree(index: DesktopIndex, options: TreeOptions): TreeRow[] {
  const { now, query, scope, provider, expanded } = options;
  const filtering = Boolean(query.trim()) || scope !== 'all' || Boolean(provider);
  const rows: TreeRow[] = [];
  const isExpanded = (id: string, fallback = true) => expanded[id] ?? fallback;
  const add = (row: Omit<TreeRow, 'highlights'>) => rows.push({ ...row, highlights: fuzzyMatch(row.label, query) ?? [] });
  const smart = smartThreads(index, now);
  const devices = new Map(index.devices.map(device => [device.id, device]));
  const workspaces = new Map(index.workspaces.map(workspace => [workspace.key, workspace]));
  const matching = (thread: DesktopThread) => {
    if (scope === 'active' && !active(thread, now)) return false;
    if (scope === 'pinned' && !thread.isPinned) return false;
    if (provider && thread.provider !== provider) return false;
    const workspace = workspaces.get(`${thread.deviceId}:${thread.workspaceId}`);
    return fuzzyMatch(`${thread.title} ${thread.agentName ?? ''} ${workspace?.label ?? ''} ${devices.get(thread.deviceId)?.name ?? ''}`, query) !== null;
  };
  for (const [key, label] of [['attention', '需要你'], ['running', '进行中'], ['completed', '刚完成']] as const) {
    const list = smart[key].filter(matching); const id = `smart:${key}`;
    add({ id, kind: 'section', label, depth: 0, parentId: null, expandable: true, expanded: isExpanded(id, false), rollup: rollup(list, now), count: list.length });
    if (isExpanded(id, false)) for (const thread of list) add({ id: `${id}:${thread.key}`, parentId: id, kind: 'thread', label: thread.title, threadKey: thread.key, deviceId: thread.deviceId, workspaceId: thread.workspaceId, depth: 1, expandable: false, expanded: false, rollup: emptyRollup() });
  }
  for (const kind of ['filter', 'scope'] as const) add({ id: `nav:${kind}`, kind, label: '', depth: 0, parentId: null, expandable: false, expanded: false, rollup: emptyRollup() });
  const threadsByWorkspace = new Map<string, DesktopThread[]>();
  for (const thread of index.threads) {
    const key = `${thread.deviceId}:${thread.workspaceId}`;
    const list = threadsByWorkspace.get(key) ?? []; list.push(thread); threadsByWorkspace.set(key, list);
  }
  for (const device of [...index.devices].sort((a, b) => Number(b.connected) - Number(a.connected) || a.name.localeCompare(b.name))) {
    const deviceThreads = index.threads.filter(thread => thread.deviceId === device.id);
    const deviceWorkspaces = index.workspaces.filter(workspace => workspace.deviceId === device.id).map(workspace => {
      const threads = (threadsByWorkspace.get(workspace.key) ?? []).sort(compareThreads);
      const latest = threads.reduce((date, thread) => thread.updatedAt > date ? thread.updatedAt : date, workspace.lastOpenedAt ?? workspace.createdAt);
      return { workspace, threads, latest, inactive: !workspace.isFavorite && !threads.some(thread => thread.isPinned || running(thread) || needsAttention(thread, now)) && now - Date.parse(latest) > 14 * 86_400_000 };
    }).filter(item => !filtering || item.threads.some(matching)).sort((a, b) => Number(b.workspace.isFavorite) - Number(a.workspace.isFavorite) || b.latest.localeCompare(a.latest) || a.workspace.key.localeCompare(b.workspace.key));
    if (filtering && !deviceWorkspaces.length) continue;
    const deviceId = `device:${device.id}`; const openDevice = filtering || isExpanded(deviceId, device.connected);
    add({ id: deviceId, kind: 'device', label: device.name, deviceId: device.id, parentId: null, depth: 0, expandable: deviceWorkspaces.length > 0, expanded: openDevice, offline: !device.connected, lastSeenAt: device.lastHeartbeatAt, rollup: rollup(deviceThreads, now) });
    if (!openDevice) continue;
    const inactiveCount = deviceWorkspaces.filter(item => item.inactive).length;
    const inactiveId = `inactive:${device.id}`;
    for (const item of deviceWorkspaces.filter(item => !item.inactive || filtering)) renderWorkspace(item, deviceId, 1);
    if (inactiveCount && !filtering) {
      const inactiveThreads = deviceWorkspaces.filter(item => item.inactive).flatMap(item => item.threads);
      add({ id: inactiveId, kind: 'inactive', label: `不活跃的工作区（${inactiveCount}）`, parentId: deviceId, deviceId: device.id, depth: 1, expandable: true, expanded: isExpanded(inactiveId, false), rollup: rollup(inactiveThreads, now) });
      if (isExpanded(inactiveId, false)) for (const item of deviceWorkspaces.filter(item => item.inactive)) renderWorkspace(item, inactiveId, 2);
    }
    function renderWorkspace({ workspace, threads }: typeof deviceWorkspaces[number], parentId: string, depth: number) {
      const id = `workspace:${workspace.key}`;
      const visible = new Set(threads.filter(matching).map(thread => thread.id));
      const byId = new Map(threads.map(thread => [thread.id, thread]));
      // Keep every ancestor of a match, including multiple delegation levels.
      for (const threadId of [...visible]) {
        let thread = byId.get(threadId); const seen = new Set<string>();
        while (thread?.parentThreadId && !seen.has(thread.id)) {
          seen.add(thread.id); visible.add(thread.parentThreadId); thread = byId.get(thread.parentThreadId);
        }
      }
      const roots = threads.filter(thread => !thread.parentThreadId || !byId.has(thread.parentThreadId));
      const children = new Map<string, DesktopThread[]>();
      for (const thread of threads) if (thread.parentThreadId && byId.has(thread.parentThreadId)) {
        const list = children.get(thread.parentThreadId) ?? []; list.push(thread); children.set(thread.parentThreadId, list);
      }
      const focusAncestors = new Set<string>();
      for (const thread of threads.filter(thread => active(thread, now))) {
        let parent = thread.parentThreadId; const seen = new Set<string>();
        while (parent && !seen.has(parent)) { seen.add(parent); focusAncestors.add(parent); parent = byId.get(parent)?.parentThreadId; }
      }
      const autoExpand = new Set<string>();
      for (const thread of threads.filter(thread => running(thread) || needsAttention(thread, now))) {
        let parent = thread.parentThreadId; const seen = new Set<string>();
        while (parent && !seen.has(parent)) { seen.add(parent); autoExpand.add(parent); parent = byId.get(parent)?.parentThreadId; }
      }
      const open = filtering || isExpanded(id);
      add({ id, parentId, depth, kind: 'workspace', label: workspace.label, workspaceId: workspace.id, deviceId: device.id, expandable: threads.length > 0, expanded: open, rollup: rollup(threads, now) });
      if (!open) return;
      const focused = new Set<string>(); let recent = 0;
      for (const root of roots) {
        const hasActiveChild = focusAncestors.has(root.id);
        if (root.isPinned || active(root, now) || hasActiveChild) focused.add(root.id);
        else if (now - Date.parse(root.updatedAt) < 7 * 86_400_000 && recent < 5) { focused.add(root.id); recent++; }
      }
      const moreId = `more:${workspace.key}`;
      for (const root of roots) if ((filtering ? visible.has(root.id) : focused.has(root.id) || isExpanded(moreId, false))) renderThread(root, id, depth + 1, new Set());
      const remaining = roots.filter(root => !focused.has(root.id)).length;
      if (!filtering && remaining) add({ id: moreId, parentId: id, kind: 'more', depth: depth + 1, label: `${isExpanded(moreId, false) ? '收起' : '… 其余'} ${remaining} 个线程`, deviceId: device.id, workspaceId: workspace.id, expandable: true, expanded: isExpanded(moreId, false), rollup: emptyRollup() });
      function descendantRollup(threadId: string, visited: Set<string>): Rollup {
        if (visited.has(threadId)) return emptyRollup();
        const nextVisited = new Set(visited).add(threadId);
        const direct = children.get(threadId) ?? [];
        return direct.reduce((sum, child) => {
          const descendants = descendantRollup(child.id, nextVisited);
          return { running: sum.running + descendants.running, attention: sum.attention + descendants.attention, unread: sum.unread + descendants.unread };
        }, rollup(direct, now));
      }
      function renderThread(thread: DesktopThread, parent: string, level: number, visited: Set<string>) {
        if (visited.has(thread.id)) return; const nextVisited = new Set(visited).add(thread.id);
        const id = `thread:${thread.key}`;
        const descendants = children.get(thread.id) ?? [];
        const open = filtering || autoExpand.has(thread.id) || isExpanded(id, false);
        add({ id, parentId: parent, depth: level, kind: 'thread', label: thread.title, threadKey: thread.key, deviceId: device.id, workspaceId: workspace.id, expandable: descendants.length > 0, expanded: open, rollup: descendantRollup(thread.id, new Set()) });
        if (!open) return;
        const closedId = `closed:${thread.key}`;
        const closed = descendants.filter(child => child.closedAt && !running(child) && !needsAttention(child, now));
        for (const child of descendants) if ((filtering ? visible.has(child.id) : !closed.includes(child) || isExpanded(closedId, false))) renderThread(child, id, level + 1, nextVisited);
        if (!filtering && closed.length) add({ id: closedId, parentId: id, kind: 'closed', label: `${isExpanded(closedId, false) ? '隐藏' : '显示'}已关闭的 ${closed.length} 个`, depth: level + 1, deviceId: device.id, expandable: true, expanded: isExpanded(closedId, false), rollup: emptyRollup() });
      }
    }
  }
  return rows;
}
/** Freeze membership and order while engaged; status fields still use newest rows. */
export function freezeRows(previous: readonly TreeRow[], desired: readonly TreeRow[], frozen: boolean): readonly TreeRow[] {
  if (!frozen) return desired;
  const next = new Map(desired.map(row => [row.id, row]));
  return previous.map(row => next.get(row.id) ?? row);
}
