import { DesktopStoreContext } from '../desktop/data/context';
import { useHostSurface } from '../desktop/surface';
import { useContext, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
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
  threadAgentKey,
  threadThemeAgentKey,
  useAppShellNav,
  type ThreadMapDevice,
  type ThreadMapPrice,
  type ThreadMapThread,
} from '@pockymoe/thread-ui';
import { translate } from '@pockymoe/thread-ui/i18n';
import { relayModeActive, request } from '../lib/api';
import { relayDeviceIdFromPath, threadHref, threadIdFromPath, threadsHref, workspacesHref } from '../lib/relayRoutes';
import { appPathname, appUrl } from '../lib/webBase';
import { workbenchThreadStatus } from '../pages/useWorkbenchNavigation';

/** The map colors threads by the time they ran in this many hours. */
export const THREAD_MAP_HOURS = 24;
/** While the map is open its devices are read again this often. */
const REFRESH_MS = 30_000;
const LOCAL = 'local';
/**
 * How long the first answers wait for the rest. Each answer changes every block's
 * share of the map, so showing them one by one makes the whole map jump.
 */
const SETTLE_MS = 1200;

/**
 * One device on the map: its workspaces and the conversations started in each,
 * the same set the sidebar's "This workspace" lists. A conversation is a root
 * thread (no `rootThreadId`) with every agent thread of its lineage folded in,
 * closed ones and ones working in another workspace included; their time and
 * tokens count on the conversation's tile. An agent thread whose conversation is
 * no longer listed (it was deleted) has no tile, as it has no sidebar row.
 * Without activity (an older Supervisor) turns, run time and the latest prompt
 * are unknown rather than zero.
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
  const family = new Map<string, ThreadDto[]>();
  for (const thread of threads) {
    if (thread.rootThreadId) family.set(thread.rootThreadId, [...(family.get(thread.rootThreadId) ?? []), thread]);
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
      agentName: agent ? themeAgentName(threadAgentKey(thread) || thread.provider, agent) : undefined,
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
      interrupted: workbenchThreadStatus(thread) === 'interrupted' || undefined,
      tokenUsage: usages.length ? Object.fromEntries(TOKENS.map(field => [field, usages.reduce((total, usage) => total + (usage[field] ?? 0), 0)])) as unknown as ThreadTurnTokenBreakdownDto : undefined,
      priceEstimate: prices.length ? Object.fromEntries(USD.map(field => [field, prices.reduce((total, price) => total + (price[field] ?? 0), 0)])) as unknown as ThreadMapPrice : undefined,
      pricePartial: memberStats.some(item => item.pricePartial) || undefined,
      agentThreads: members.length - 1 || undefined,
    };
  };
  const byWorkspace = new Map<string, ThreadMapThread[]>();
  for (const thread of threads) {
    if (thread.rootThreadId) continue;
    const children = family.get(thread.id) ?? [];
    const tile = entry(thread, [thread, ...children]);
    // Each agent thread on its own, for opening the root's tile up into them.
    if (children.length) tile.children = children.map(child => entry(child, [child]));
    byWorkspace.set(thread.workspaceId, [...(byWorkspace.get(thread.workspaceId) ?? []), tile]);
  }
  const labels = new Map(workspaces.map(workspace => [workspace.id, workspace]));
  return {
    id: key,
    name,
    href: appUrl(workspacesHref(deviceId)),
    state: 'ready',
    workspaces: [...byWorkspace].map(([id, list]) => ({
      id,
      label: labels.get(id)?.label ?? id.slice(0, 8),
      href: appUrl(threadsHref(id, deviceId)),
      path: labels.get(id)?.absPath,
      threads: list,
    })),
  };
}

/**
 * Reads one device's map. The thread list answers at once while activity can take
 * seconds, so `onThreads` gets a map sized alike first and the result has activity.
 */
async function readDevice(deviceId: string | null, name: string, onThreads?: (device: ThreadMapDevice) => void): Promise<ThreadMapDevice> {
  const base = deviceId ? `relay/devices/${encodeURIComponent(deviceId)}/` : '';
  const read = <T,>(path: string) => request<T>(`${base}${path}`, { signal: AbortSignal.timeout(15_000), cache: 'no-store' });
  try {
    // Older Supervisors have no activity; the map then sizes threads alike.
    const activityRead = read<ThreadActivityResponseDto>(`api/thread-activity?hours=${THREAD_MAP_HOURS}`).catch(() => null);
    const [workspaces, threads] = await Promise.all([
      read<WorkspaceDto[]>('api/workspaces'),
      read<ThreadDto[]>('api/threads?includeAgentThreads=true'),
    ]);
    if (!Array.isArray(workspaces) || !Array.isArray(threads)) throw new Error('Invalid thread list');
    onThreads?.(mapDevice(deviceId, name, workspaces, threads, null));
    const activity = await activityRead;
    return mapDevice(deviceId, name, workspaces, threads, activity && Array.isArray(activity.threads) ? activity : null);
  } catch (error) {
    return { id: deviceId ?? LOCAL, name, state: 'error', error: error instanceof Error ? error.message : String(error), workspaces: [] };
  }
}

type DeviceEntry = { id: string | null; name: string; connected: boolean };

/** The account's own devices (or this one, locally), read while the map is open. */
export function useThreadMapDevices(open: boolean) {
  const sharedIndex = useContext(DesktopStoreContext);
  const [devices, setDevices] = useState<ThreadMapDevice[]>([]);
  useEffect(() => {
    if (!sharedIndex || !open) return;
    sharedIndex.setMapVisible(true);
    const sync = () => {
      const index = sharedIndex.getSnapshot();
      setDevices(index.devices.map(device => ({ ...mapDevice(device.id, device.name,
        index.workspaces.filter(workspace => workspace.deviceId === device.id), index.threads.filter(thread => thread.deviceId === device.id), index.activity[device.id] ?? null),
        state: device.connected ? device.cachedAt ? 'ready' : 'loading' : 'offline' })));
    };
    sync(); const unsubscribe = sharedIndex.subscribe(sync);
    return () => { unsubscribe(); sharedIndex.setMapVisible(false); };
  }, [sharedIndex, open]);
  const entries = useRef<DeviceEntry[]>([]);
  const generation = useRef(0);
  const settle = useCallback((run: number, answers: readonly ThreadMapDevice[]) => {
    if (run !== generation.current || !answers.length) return;
    // A refresh that fails keeps what was already on the map.
    setDevices(list => list.map(device => {
      const next = answers.find(answer => answer.id === device.id);
      return !next ? device : next.state === 'error' && device.state === 'ready' ? device : next;
    }));
  }, []);
  const refresh = useCallback(async () => {
    const run = ++generation.current;
    let list: DeviceEntry[];
    if (relayModeActive()) {
      try {
        const portal = await request<RelayPortalSummaryDto>('relay/portal', { signal: AbortSignal.timeout(15_000), cache: 'no-store' });
        list = portal.devices.filter(device => device.connected).map(device => ({ id: device.id, name: device.name, connected: true }));
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
      return known && known.state !== 'offline' ? { ...known, name: entry.name } : { id, name: entry.name, state: 'loading', workspaces: [] };
    }));
    // Answers land together. One slow device must not hold back the rest for long, so after
    // the wait they appear, and whatever is still out lands in the same way when it can.
    const waiting: ThreadMapDevice[] = [];
    let timer: ReturnType<typeof setTimeout> | undefined;
    const flush = () => { timer = undefined; settle(run, waiting.splice(0)); };
    // Devices still loading show their threads before activity arrives; these
    // early maps land together too, once all have answered or after the wait.
    const connected = list.filter(entry => entry.connected);
    const earlyWaiting: ThreadMapDevice[] = [];
    let earlyCount = 0;
    let earlyTimer: ReturnType<typeof setTimeout> | undefined;
    const flushEarly = () => {
      clearTimeout(earlyTimer); earlyTimer = undefined;
      const answers = earlyWaiting.splice(0);
      if (run !== generation.current || !answers.length) return;
      setDevices(current => current.map(known => known.state === 'loading' ? answers.find(answer => answer.id === known.id) ?? known : known));
    };
    const early = (device: ThreadMapDevice) => {
      earlyWaiting.push(device);
      if (++earlyCount === connected.length) flushEarly();
      else earlyTimer ??= setTimeout(flushEarly, SETTLE_MS);
    };
    await Promise.all(connected.map(async entry => {
      waiting.push(await readDevice(entry.id, entry.name, early));
      timer ??= setTimeout(flush, SETTLE_MS);
    }));
    clearTimeout(timer);
    clearTimeout(earlyTimer);
    earlyWaiting.length = 0;
    flush();
  }, [settle]);
  const retry = useCallback((id: string) => {
    if (sharedIndex) { void sharedIndex.refreshDevice(id); return; }
    const entry = entries.current.find(item => (item.id ?? LOCAL) === id);
    if (!entry) return;
    const run = generation.current;
    setDevices(list => list.map(device => device.id === id ? { ...device, state: 'loading', error: null } : device));
    void readDevice(entry.id, entry.name).then(next => settle(run, [next]));
  }, [settle, sharedIndex]);
  useEffect(() => {
    if (!open || sharedIndex) return;
    void refresh();
    const timer = window.setInterval(() => {
      if (document.visibilityState === 'visible') void refresh();
    }, REFRESH_MS);
    return () => {
      window.clearInterval(timer);
      generation.current++;
    };
  }, [open, refresh, sharedIndex]);
  return { devices, retry };
}

function ThreadMapLayer({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const shell = useAppShellNav();
  const surface = useHostSurface();
  const navigate = useNavigate();
  const location = useLocation();
  const { devices, retry } = useThreadMapDevices(open);
  const threadId = threadIdFromPath(location.pathname);
  const currentKey = threadId ? `${relayDeviceIdFromPath(location.pathname) ?? LOCAL}:${threadId}` : undefined;
  return (
    <ThreadMapDialog hostSurface={surface === 'pending' ? 'browser' : surface} open={open} onOpenChange={onOpenChange} devices={devices} currentKey={currentKey}
      hours={THREAD_MAP_HOURS} themeMode={shell?.themeMode} effectiveTheme={shell?.effectiveTheme}
      onRetry={retry}
      onNavigate={href => {
        onOpenChange(false);
        navigate(appPathname(href));
      }}
      onOpen={thread => {
        onOpenChange(false);
        navigate(appPathname(thread.href));
      }} />
  );
}

/**
 * Offers the thread map to the app (`openThreadMap` on the shell context) and
 * renders it. In a Treer Agent UI its one root is this hub: the hub answers the
 * workspace, thread and activity reads with the threads that tunnel may open.
 */
export function ThreadMapProvider({ children }: { children: ReactNode }) {
  const shell = useAppShellNav();
  const [open, setOpen] = useState(false);
  const value = useMemo(() => (shell ? { ...shell, openThreadMap: () => setOpen(true) } : shell), [shell]);
  return (
    <AppShellNavContext.Provider value={value}>
      {children}
      <ThreadMapLayer open={open} onOpenChange={setOpen} />
    </AppShellNavContext.Provider>
  );
}
