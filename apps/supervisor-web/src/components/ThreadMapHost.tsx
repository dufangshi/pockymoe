import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import type {
  RelayPortalSummaryDto,
  ThreadActivityDto,
  ThreadActivityResponseDto,
  ThreadDto,
  ThreadTurnTokenBreakdownDto,
  WorkspaceDto,
} from '@pockymoe/shared';
import {
  AppShellNavContext,
  ThreadMapDialog,
  themeAgentName,
  threadThemeAgentKey,
  useAppShellNav,
  type ThreadMapDevice,
  type ThreadMapPrice,
  type ThreadMapThread,
} from '@pockymoe/thread-ui';
import { translate } from '@pockymoe/thread-ui/i18n';
import { relayModeActive, request } from '../lib/api';
import { useHostScope } from '../lib/hostCapabilities';
import { relayDeviceIdFromPath, threadHref, threadIdFromPath } from '../lib/relayRoutes';
import { appPathname, appUrl } from '../lib/webBase';
import { workbenchThreadStatus } from '../pages/useWorkbenchNavigation';

/** The map colors threads by the time they ran in this many hours. */
export const THREAD_MAP_HOURS = 24;
/** While the map is open its devices are read again this often. */
const REFRESH_MS = 30_000;
const LOCAL = 'local';

/**
 * One device on the map: its workspaces and their root threads, each with its
 * agent threads folded in. An agent thread working in another workspace than its
 * conversation has a tile of its own there, with the agent threads it started in
 * that workspace. Without activity (an older Supervisor) turns, run time and the
 * latest prompt are unknown rather than zero.
 */
export function mapDevice(
  deviceId: string | null,
  name: string,
  workspaces: readonly WorkspaceDto[],
  threads: readonly ThreadDto[],
  activity: ThreadActivityResponseDto | null,
): ThreadMapDevice {
  const key = deviceId ?? LOCAL;
  const stats = new Map(activity?.threads.map(entry => [entry.threadId, entry]));
  const byId = new Map(threads.map(thread => [thread.id, thread]));
  // A thread's tile is its topmost listed ancestor in the same workspace: the root
  // for agent threads working where their conversation started. An agent thread
  // with no such ancestor (its conversation is elsewhere or gone) leads its own.
  const tileOf = new Map<string, ThreadDto>();
  for (const thread of threads) {
    let owner = thread;
    let parent = thread.parentThreadId ? byId.get(thread.parentThreadId) : undefined;
    // Lineages are a few levels deep; the bound only guards bad data.
    for (let depth = 0; parent && depth < 16; depth++) {
      if (parent.workspaceId === thread.workspaceId) owner = parent;
      parent = parent.parentThreadId ? byId.get(parent.parentThreadId) : undefined;
    }
    const root = thread.rootThreadId ? byId.get(thread.rootThreadId) : undefined;
    if (root && root.id !== thread.id && root.workspaceId === thread.workspaceId) owner = root;
    tileOf.set(thread.id, owner);
  }
  const family = new Map<string, ThreadDto[]>();
  for (const thread of threads) {
    const owner = tileOf.get(thread.id)!;
    if (owner !== thread) family.set(owner.id, [...(family.get(owner.id) ?? []), thread]);
  }
  const TOKENS = ['totalTokens', 'inputTokens', 'cachedInputTokens', 'cacheWriteInputTokens', 'outputTokens', 'reasoningOutputTokens'] as const;
  const USD = ['totalUsd', 'inputUsd', 'cachedInputUsd', 'cacheWriteInputUsd', 'outputUsd'] as const;
  // One tile: a thread with the given members (itself and, for a root, its agent threads) summed.
  const entry = (thread: ThreadDto, members: ThreadDto[]): ThreadMapThread => {
    const agent = threadThemeAgentKey(thread);
    const own = stats.get(thread.id);
    const memberStats = members.map(member => stats.get(member.id)).filter((item): item is ThreadActivityDto => Boolean(item));
    const sum = (field: 'turns' | 'activeMs' | 'totalMs') => memberStats.reduce((total, item) => total + (item[field] ?? 0), 0);
    const hourly = memberStats.map(item => item.hourlyMs).filter((list): list is number[] => Array.isArray(list));
    const usages = memberStats.map(item => item.tokenUsage).filter((usage): usage is ThreadTurnTokenBreakdownDto => Boolean(usage));
    const prices = memberStats.map(item => item.priceEstimate).filter((price): price is NonNullable<ThreadActivityDto['priceEstimate']> => Boolean(price));
    const lastActive = members.flatMap(member => [member.lastTurnStartedAt, member.lastTurnCompletedAt])
      .filter((at): at is string => Boolean(at)).sort().at(-1);
    return {
      key: `${key}:${thread.id}`,
      title: thread.title.trim() || translate('workbench.splitUntitled'),
      href: appUrl(threadHref(thread.id, deviceId)),
      agent,
      agentName: agent ? themeAgentName(thread.agentId || thread.provider, agent) : undefined,
      running: members.some(member => workbenchThreadStatus(member) === 'running'),
      turns: activity ? sum('turns') : null,
      activeMs: activity ? sum('activeMs') : null,
      totalMs: activity ? sum('totalMs') : null,
      hourlyMs: hourly.length ? hourly[0]!.map((_, hour) => hourly.reduce((total, list) => total + (list[hour] ?? 0), 0)) : undefined,
      createdAt: thread.createdAt,
      lastActiveAt: lastActive,
      model: thread.model ?? undefined,
      lastPrompt: own?.lastPrompt,
      failed: workbenchThreadStatus(thread) === 'failed' || undefined,
      tokenUsage: usages.length ? Object.fromEntries(TOKENS.map(field => [field, usages.reduce((total, usage) => total + (usage[field] ?? 0), 0)])) as unknown as ThreadTurnTokenBreakdownDto : undefined,
      priceEstimate: prices.length ? Object.fromEntries(USD.map(field => [field, prices.reduce((total, price) => total + (price[field] ?? 0), 0)])) as unknown as ThreadMapPrice : undefined,
      pricePartial: memberStats.some(item => item.pricePartial) || undefined,
      agentThreads: members.length - 1 || undefined,
    };
  };
  const byWorkspace = new Map<string, ThreadMapThread[]>();
  for (const thread of threads) {
    if (tileOf.get(thread.id) !== thread) continue;
    const children = family.get(thread.id) ?? [];
    const tile = entry(thread, [thread, ...children]);
    // Each agent thread on its own, for opening the root's tile up into them.
    if (children.length) tile.children = children.map(child => entry(child, [child]));
    // An agent thread on a tile of its own says which thread started it.
    const starter = byId.get(thread.parentThreadId ?? '') ?? byId.get(thread.rootThreadId ?? '');
    if (starter && starter !== thread) tile.from = starter.title.trim() || translate('workbench.splitUntitled');
    byWorkspace.set(thread.workspaceId, [...(byWorkspace.get(thread.workspaceId) ?? []), tile]);
  }
  const labels = new Map(workspaces.map(workspace => [workspace.id, workspace]));
  return {
    id: key,
    name,
    state: 'ready',
    workspaces: [...byWorkspace].map(([id, list]) => ({
      id,
      label: labels.get(id)?.label ?? id.slice(0, 8),
      path: labels.get(id)?.absPath,
      threads: list,
    })),
  };
}

async function readDevice(deviceId: string | null, name: string): Promise<ThreadMapDevice> {
  const base = deviceId ? `relay/devices/${encodeURIComponent(deviceId)}/` : '';
  const read = <T,>(path: string) => request<T>(`${base}${path}`, { signal: AbortSignal.timeout(15_000), cache: 'no-store' });
  try {
    const [workspaces, threads, activity] = await Promise.all([
      read<WorkspaceDto[]>('api/workspaces'),
      read<ThreadDto[]>('api/threads?includeAgentThreads=true'),
      // Older Supervisors have no activity; the map then sizes threads alike.
      read<ThreadActivityResponseDto>(`api/thread-activity?hours=${THREAD_MAP_HOURS}`).catch(() => null),
    ]);
    if (!Array.isArray(workspaces) || !Array.isArray(threads)) throw new Error('Invalid thread list');
    return mapDevice(deviceId, name, workspaces, threads, activity && Array.isArray(activity.threads) ? activity : null);
  } catch (error) {
    return { id: deviceId ?? LOCAL, name, state: 'error', error: error instanceof Error ? error.message : String(error), workspaces: [] };
  }
}

type DeviceEntry = { id: string | null; name: string; connected: boolean };

/** The account's own devices (or this one, locally), read while the map is open. */
export function useThreadMapDevices(open: boolean) {
  const [devices, setDevices] = useState<ThreadMapDevice[]>([]);
  const entries = useRef<DeviceEntry[]>([]);
  const generation = useRef(0);
  const settle = useCallback((run: number, next: ThreadMapDevice) => {
    if (run !== generation.current) return;
    // A refresh that fails keeps what was already on the map.
    setDevices(list => list.map(device => device.id !== next.id ? device
      : next.state === 'error' && device.state === 'ready' ? device : next));
  }, []);
  const refresh = useCallback(async () => {
    const run = ++generation.current;
    let list: DeviceEntry[];
    if (relayModeActive()) {
      try {
        const portal = await request<RelayPortalSummaryDto>('relay/portal', { signal: AbortSignal.timeout(15_000), cache: 'no-store' });
        list = portal.devices.map(device => ({ id: device.id, name: device.name, connected: device.connected }));
      } catch {
        list = entries.current;
      }
    } else {
      list = [{ id: null, name: translate('workbench.threadMapThisDevice'), connected: true }];
    }
    if (run !== generation.current) return;
    entries.current = list;
    setDevices(previous => list.map(entry => {
      const id = entry.id ?? LOCAL;
      const known = previous.find(device => device.id === id);
      if (!entry.connected) return { id, name: entry.name, state: 'offline', workspaces: [] };
      return known && known.state !== 'offline' ? { ...known, name: entry.name } : { id, name: entry.name, state: 'loading', workspaces: [] };
    }));
    await Promise.all(list.filter(entry => entry.connected).map(async entry => settle(run, await readDevice(entry.id, entry.name))));
  }, [settle]);
  const retry = useCallback((id: string) => {
    const entry = entries.current.find(item => (item.id ?? LOCAL) === id);
    if (!entry) return;
    const run = generation.current;
    setDevices(list => list.map(device => device.id === id ? { ...device, state: 'loading', error: null } : device));
    void readDevice(entry.id, entry.name).then(next => settle(run, next));
  }, [settle]);
  useEffect(() => {
    if (!open) return;
    void refresh();
    const timer = window.setInterval(() => {
      if (document.visibilityState === 'visible') void refresh();
    }, REFRESH_MS);
    return () => {
      window.clearInterval(timer);
      generation.current++;
    };
  }, [open, refresh]);
  return { devices, retry };
}

function ThreadMapLayer({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const shell = useAppShellNav();
  const navigate = useNavigate();
  const location = useLocation();
  const { devices, retry } = useThreadMapDevices(open);
  const threadId = threadIdFromPath(location.pathname);
  const currentKey = threadId ? `${relayDeviceIdFromPath(location.pathname) ?? LOCAL}:${threadId}` : undefined;
  return (
    <ThreadMapDialog open={open} onOpenChange={onOpenChange} devices={devices} currentKey={currentKey}
      hours={THREAD_MAP_HOURS} themeMode={shell?.themeMode} effectiveTheme={shell?.effectiveTheme}
      onRetry={retry}
      onOpen={thread => {
        onOpenChange(false);
        navigate(appPathname(thread.href));
      }} />
  );
}

/**
 * Offers the thread map to the app (`openThreadMap` on the shell context) and
 * renders it. A host-scoped Supervisor shows only part of a device, so it has none.
 */
export function ThreadMapProvider({ children }: { children: ReactNode }) {
  const shell = useAppShellNav();
  const scoped = useHostScope();
  const [open, setOpen] = useState(false);
  const value = useMemo(() => (shell && !scoped ? { ...shell, openThreadMap: () => setOpen(true) } : shell), [shell, scoped]);
  return (
    <AppShellNavContext.Provider value={value}>
      {children}
      {!scoped && <ThreadMapLayer open={open} onOpenChange={setOpen} />}
    </AppShellNavContext.Provider>
  );
}
