import { useBrowserStateKey } from '../lib/browserStateScope';
import { narrationQuery } from '../lib/transcriptPreferences';
import { useHostScope } from '../lib/hostCapabilities';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { ThreadDetailDto, ThreadDto, WorkspaceDto } from '@pockymoe/shared';
import type {
  WorkbenchNotification,
  WorkbenchThread,
} from '@pockymoe/thread-ui';
import { ApiError, isInvisibleThreadError, relayModeActive, request } from '../lib/api';
import { threadHref } from '../lib/relayRoutes';
import { threadThemeAgentKey } from '@pockymoe/thread-ui';

/** The avatar key a theme shows for a thread's harness. */
function threadAgentField(thread: { provider?: string | null; agentId?: string | null }): { agent?: string } {
  const agent = threadThemeAgentKey(thread);
  return agent ? { agent } : {};
}

interface ThreadReference {
  deviceId: string | null;
  threadId: string;
  title: string;
  workspaceLabel: string;
  workspaceId?: string;
  deviceName?: string;
  favorite: boolean;
  /** The account's own shortcut order; absent until shortcuts are reordered. */
  favoriteOrder?: number | null;
  visitedAt: string;
  readCompletedAt?: string | null;
}
type ThreadActivity = { status: string; lastTurnCompletedAt?: string | null; parentThreadId?: string | null; rootThreadId?: string | null };
export function workbenchThreadStatus(activity: ThreadActivity | undefined, readCompletedAt?: string | null) {
  if (!activity) return 'unknown';
  if (['running', 'inProgress', 'recovering'].includes(activity.status)) return 'running';
  if (['failed', 'error', 'system_error'].includes(activity.status)) return 'failed';
  if (['unknown', 'not_loaded'].includes(activity.status)) return 'unknown';
  if (activity.status === 'interrupted') return 'interrupted';
  if (activity.lastTurnCompletedAt && (!readCompletedAt || Date.parse(activity.lastTurnCompletedAt) > Date.parse(readCompletedAt))) return 'unread';
  return 'idle';
}
interface NavigationSnapshot {
  threads: ThreadReference[];
  notifications: WorkbenchNotification[];
}
const LOCAL_KEY = 'remote-codex.local-workbench.v1';
const WORKSPACE_ORDER_KEY = 'pockymoe.workspace-order.v1';
function loadWorkspaceOrders(key: string): Record<string, string[]> {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(key) ?? '{}');
    if (!value || typeof value !== 'object') return {};
    return Object.fromEntries(Object.entries(value).filter((entry): entry is [string, string[]] => Array.isArray(entry[1]) && entry[1].every(item => typeof item === 'string')));
  } catch { return {}; }
}
const referenceKey = (r: { deviceId: string | null; threadId: string }) =>
  `${r.deviceId ?? 'local'}:${r.threadId}`;
function loadLocal(storageKey: string): NavigationSnapshot {
  try {
    const value = JSON.parse(
      localStorage.getItem(storageKey) ?? '{}',
    ) as NavigationSnapshot;
    return {
      threads: Array.isArray(value.threads) ? value.threads : [],
      notifications: Array.isArray(value.notifications)
        ? value.notifications
        : [],
    };
  } catch {
    return { threads: [], notifications: [] };
  }
}

/** Navigation state per browser scope, kept across thread switches (page remounts). */
const lastNavigation = new Map<string, { snapshot: NavigationSnapshot; statuses: Record<string, ThreadActivity>; families: Record<string, ThreadDto[]> }>();

/** Relay navigation re-reads other devices this often while visible. */
export const RELAY_NAVIGATION_REFRESH_MS = 30_000;

export function useWorkbenchNavigation(
  detail: ThreadDetailDto | null,
  threads: ThreadDto[],
  deviceId: string | null,
  routeThreadId?: string,
  options: {
    /**
     * `threads` is the caller's live list of every thread on `deviceId`
     * (including agent threads), so navigation does not fetch it again.
     */
    sharesDeviceThreads?: boolean;
  } = {},
) {
  const hostScope = useHostScope();
  const storageKey = useBrowserStateKey(LOCAL_KEY);
  // The order the reader dragged each workspace's conversations into, kept in
  // this browser: `device:workspace` -> root thread keys, first to last.
  const orderKey = useBrowserStateKey(WORKSPACE_ORDER_KEY);
  const [workspaceOrders, setWorkspaceOrders] = useState<Record<string, string[]>>(() => loadWorkspaceOrders(orderKey));
  useEffect(() => { setWorkspaceOrders(loadWorkspaceOrders(orderKey)); }, [orderKey]);
  const sharesDeviceThreads = options.sharesDeviceThreads === true;
  const threadsRef = useRef(threads);
  threadsRef.current = threads;
  useI18n();
  const relay = !hostScope && relayModeActive();
  // Thread switches remount the page: start from the last navigation state seen
  // in this browser scope (favorites, read marks, statuses) until it refreshes.
  const cacheKey = `${relay ? 'relay' : 'local'}:${storageKey}`;
  const cached = lastNavigation.get(cacheKey);
  const [snapshot, setSnapshot] = useState<NavigationSnapshot>(() => cached?.snapshot ?? {
    threads: [],
    notifications: [],
  });
  const [statuses, setStatuses] = useState<Record<string, ThreadActivity>>(() => cached?.statuses ?? {});
  const [families, setFamilies] = useState<Record<string, ThreadDto[]>>(() => cached?.families ?? {});
  useEffect(() => { lastNavigation.set(cacheKey, { snapshot, statuses, families }); }, [cacheKey, snapshot, statuses, families]);
  // Every thread listed by the devices navigation read, for the running sheet:
  // running threads count wherever they are, opened before or not.
  const [deviceLists, setDeviceLists] = useState<Record<string, ThreadDto[]>>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [navigationReady, setNavigationReady] = useState(false);
  const [readAt, setReadAt] = useState(Date.now());
  const [notificationDetails, setNotificationDetails] = useState<Record<string, { title: string; summary: string; status: 'unread' | 'failed' | 'interrupted' }>>({});
  const [notificationsOpened, setNotificationsOpened] = useState(0);
  // Renames and deletions made from the sidebar, shown before the device's
  // thread list catches up: a rename holds while the list keeps the old title.
  const [localEdits, setLocalEdits] = useState<{ renamed: Record<string, { from: string; to: string }>; removed: string[] }>({ renamed: {}, removed: [] });
  // Workspace labels read from the device, once per workspace the tabs need and
  // nothing else names (a conversation here working in another workspace).
  const [readLabels, setReadLabels] = useState<{ device: string; labels: Record<string, string>; tried: string[] }>({ device: '', labels: {}, tried: [] });
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const revision = useRef(0);
  const currentKey = `${deviceId ?? 'local'}:${routeThreadId ?? detail?.thread.id ?? ''}`;
  const current = useRef({ detail, deviceId });
  current.current = { detail, deviceId };
  const navigationContext = useRef({ detail, threads, deviceId });
  if (detail) navigationContext.current = { detail, threads, deviceId };
  const tabContext = detail ? { detail, threads, deviceId } : navigationContext.current;
  const threadId = detail?.thread.id;

  useEffect(() => {
    let alive = true;
    let inFlight = false;
    const controller = new AbortController();
    async function refresh() {
      if (inFlight || document.visibilityState === 'hidden') return;
      inFlight = true;
      const startedAtRevision = revision.current;
      try {
        const next = relay
          ? await request<NavigationSnapshot>('relay/account/workbench', {
              signal: controller.signal,
            })
          : loadLocal(storageKey);
        if (!relay && hostScope) {
          const missing = new Set<string>();
          await Promise.all(next.threads.map(async r => {
            if (r.deviceId !== null) { missing.add(referenceKey(r)); return; }
            if (threadsRef.current.some(t => t.id === r.threadId)) return;
            try { await request(`api/threads/${encodeURIComponent(r.threadId)}?view=summary&limit=1&compact=1${narrationQuery()}`, { signal: controller.signal }); }
            catch (error) { if (isInvisibleThreadError(error)) missing.add(referenceKey(r)); }
          }));
          if (alive && startedAtRevision === revision.current && missing.size) {
            next.threads = next.threads.filter(r => !missing.has(referenceKey(r)));
            localStorage.setItem(storageKey, JSON.stringify(next));
          }
        }
        if (!alive) return;
        if (startedAtRevision === revision.current) { setSnapshot(next); setNavigationReady(true); }
        setError(null);
        if (relay) {
          // Explicit device + thread paths: never switch the browser's selected device to poll.
          const deviceSnapshots = new Map<string, ThreadDto[]>();
          // The page keeps the current device's list current from its events.
          const sharedDevice = sharesDeviceThreads ? deviceId : null;
          if (sharedDevice && threadsRef.current.length) deviceSnapshots.set(sharedDevice, threadsRef.current);
          const pendingDevices = [...new Set(next.threads.map(r => r.deviceId).filter((id): id is string => !!id && id !== sharedDevice))];
          await Promise.all(Array.from({ length: Math.min(6, pendingDevices.length) }, async () => {
            while (alive && pendingDevices.length) {
              const id = pendingDevices.shift()!;
              try {
                const list = await request<ThreadDto[]>(
                  `relay/devices/${encodeURIComponent(id)}/api/threads?includeAgentThreads=true`,
                  { signal: AbortSignal.any([controller.signal, AbortSignal.timeout(5000)]), cache: 'no-store' },
                );
                if (Array.isArray(list)) deviceSnapshots.set(id, list);
              } catch { /* Thread-scoped access falls back to summary/group endpoints below. */ }
            }
          }));
          // Until the shared list has loaded, its threads wait for the next pass
          // instead of one summary request each.
          const pending = next.threads.filter(r => !sharedDevice || r.deviceId !== sharedDevice || deviceSnapshots.has(sharedDevice));
          const missing: ThreadReference[] = [];
          const updates: Record<string, ThreadActivity> = {};
          const groups = new Map<string, ThreadReference>();
          await Promise.all(
            Array.from({ length: Math.min(6, pending.length) }, async () => {
              while (alive && pending.length) {
                const r = pending.shift()!;
                try {
                  const listed = deviceSnapshots.get(r.deviceId!)?.find(thread => thread.id === r.threadId);
                  const value = listed ? { thread: listed } : await request<ThreadDetailDto>(
                    `relay/devices/${encodeURIComponent(r.deviceId!)}/api/threads/${encodeURIComponent(r.threadId)}?view=summary&limit=1&compact=1${narrationQuery()}`,
                    {
                      signal: AbortSignal.any([
                        controller.signal,
                        AbortSignal.timeout(5000),
                      ]),
                    },
                  );
                  updates[referenceKey(r)] = value.thread;
                  const root = { ...r, threadId: value.thread.rootThreadId ?? r.threadId };
                  groups.set(referenceKey(root), root);
                } catch (error) {
                  if (error instanceof ApiError && error.statusCode === 404 && /thread.*not found/i.test(error.message)) missing.push(r);
                  updates[referenceKey(r)] = { status: 'unknown' };
                }
              }
            }),
          );
          // Visits are shortcuts, not complete thread groups. Query each visible
          // family once, including on devices/workspaces that are not selected.
          // The group endpoint also respects thread-scoped sharing permissions.
          const familyUpdates: Record<string, ThreadDto[]> = {};
          const pendingGroups = [...groups.values()].filter(root => {
            const list = deviceSnapshots.get(root.deviceId!);
            if (!list) return true;
            familyUpdates[referenceKey(root)] = list.filter(thread => thread.id === root.threadId || thread.rootThreadId === root.threadId);
            return false;
          });
          await Promise.all(Array.from({ length: Math.min(6, pendingGroups.length) }, async () => {
            while (alive && pendingGroups.length) {
              const root = pendingGroups.shift()!;
              try {
                const family = await request<ThreadDto[]>(
                  `relay/devices/${encodeURIComponent(root.deviceId!)}/api/threads/${encodeURIComponent(root.threadId)}/group`,
                  { signal: AbortSignal.any([controller.signal, AbortSignal.timeout(5000)]), cache: 'no-store' },
                );
                if (Array.isArray(family)) familyUpdates[referenceKey(root)] = family;
              } catch { /* Preserve the last family while its device is unavailable. */ }
            }
          }));
          if (alive) {
            setStatuses(previous => Object.fromEntries(Object.entries(updates).map(([key, activity]) => [key,
              activity.status === 'unknown' ? { ...previous[key], ...activity } : activity,
            ])));
            setFamilies(previous => ({ ...previous, ...familyUpdates }));
            setDeviceLists(previous => ({ ...previous, ...Object.fromEntries(deviceSnapshots) }));
            // Only a definitive response from the addressed device can remove
            // stale navigation. Offline, timeouts and access errors preserve it.
            for (const r of missing) {
              if (!alive) break;
              await request('relay/account/workbench', { method: 'DELETE', body: JSON.stringify({ deviceId: r.deviceId, threadId: r.threadId }), signal: controller.signal });
              if (alive) {
                ++revision.current;
                setSnapshot(previous => ({ ...previous, threads: previous.threads.filter(t => referenceKey(t) !== referenceKey(r)) }));
              }
            }
          }
        }
      } catch (e) {
        if (alive)
          setError(
            e instanceof Error ? e.message : translate("workbench.navigationCouldNotRefresh"),
          );
      } finally {
        inFlight = false;
      }
    }
    void refresh();
    // Local navigation is localStorage only; relay navigation crosses devices.
    const interval = window.setInterval(() => void refresh(), relay ? RELAY_NAVIGATION_REFRESH_MS : 5000);
    const onVisible = () => {
      if (document.visibilityState === 'visible') void refresh();
    };
    document.addEventListener('visibilitychange', onVisible);
    window.addEventListener('storage', onVisible);
    return () => {
      alive = false;
      controller.abort();
      clearInterval(interval);
      document.removeEventListener('visibilitychange', onVisible);
      window.removeEventListener('storage', onVisible);
    };
  }, [relay, deviceId, sharesDeviceThreads, storageKey, hostScope]);

  const save = useCallback(
    async (favorite?: boolean, markRead = false, target?: ThreadReference) => {
      const value = current.current.deviceId === deviceId ? current.current.detail : null;
      if (!value && !target) return;
      const writeRevision = ++revision.current;
      const reference = {
        deviceId: target ? target.deviceId : deviceId,
        threadId: target?.threadId ?? value!.thread.id,
        title: target?.title ?? value!.thread.title,
        ...((target ? target.workspaceId : value?.thread.workspaceId) ? { workspaceId: (target ? target.workspaceId : value?.thread.workspaceId)! } : {}),
        workspaceLabel: target?.workspaceLabel ?? value!.workspace.label,
        ...(favorite !== undefined ? { favorite } : {}),
        ...(markRead && value?.thread.lastTurnCompletedAt ? { readCompletedAt: value.thread.lastTurnCompletedAt } : {}),
      };
      if (relay) {
        const next = await request<NavigationSnapshot>(
          'relay/account/workbench',
          { method: 'POST', body: JSON.stringify(reference) },
        );
        if (writeRevision === revision.current) { setSnapshot(next); setNavigationReady(true); }
      } else {
        const next = loadLocal(storageKey);
        const previous = next.threads.find(
          (t) => referenceKey(t) === referenceKey(reference),
        );
        const starred = favorite ?? previous?.favorite ?? false;
        // As on the relay: a new shortcut goes after the ordered ones.
        const orders = next.threads.filter(t => t.favorite && typeof t.favoriteOrder === 'number').map(t => t.favoriteOrder!);
        const favoriteOrder = !starred ? null
          : previous?.favorite ? previous.favoriteOrder ?? null
          : orders.length ? Math.max(...orders) + 1 : null;
        const record: ThreadReference = {
          ...reference,
          favorite: starred,
          favoriteOrder,
          visitedAt: new Date().toISOString(),
          readCompletedAt: reference.readCompletedAt ?? previous?.readCompletedAt ?? null,
        };
        next.threads = [
          record,
          ...next.threads.filter(
            (t) => referenceKey(t) !== referenceKey(record),
          ),
        ].slice(0, 100);
        localStorage.setItem(storageKey, JSON.stringify(next));
        setSnapshot(next);
        setNavigationReady(true);
      }
    },
    [deviceId, relay, storageKey],
  );
  useEffect(() => {
    if (!threadId || (routeThreadId && routeThreadId !== threadId)) return;
    const visit = () => {
      if (document.visibilityState === 'hidden' || !document.hasFocus()) return;
      void save(undefined, true).catch((e) =>
      setError(
        e instanceof ApiError ? e.message : translate("workbench.couldNotSaveRecentThread"),
      ),
    );
    };
    visit();
    window.addEventListener('focus', visit);
    document.addEventListener('visibilitychange', visit);
    return () => {
      window.removeEventListener('focus', visit);
      document.removeEventListener('visibilitychange', visit);
    };
  }, [threadId, routeThreadId, detail?.thread.title, detail?.thread.lastTurnCompletedAt, save]);

  const favorite =
    snapshot.threads.find((t) => referenceKey(t) === currentKey)?.favorite ??
    false;
  const toggleFavorite = async (key = currentKey) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const target = snapshot.threads.find(t => referenceKey(t) === key);
      await save(!(target?.favorite ?? favorite), false, target);
    } catch (e) {
      setError(e instanceof Error ? e.message : translate("workbench.couldNotSaveShortcut"));
    } finally {
      setBusy(false);
    }
  };
  /** Shortcuts in a new order (thread keys, first to last), for this account. */
  const reorderShortcuts = async (keys: string[]) => {
    setError(null);
    const position = new Map(keys.map((key, index) => [key, index]));
    const reorder = (value: NavigationSnapshot): NavigationSnapshot => ({
      ...value,
      threads: value.threads.map(t => position.has(referenceKey(t)) && t.favorite ? { ...t, favoriteOrder: position.get(referenceKey(t))! } : t),
    });
    const previous = snapshot;
    setSnapshot(reorder(snapshot));
    try {
      if (relay) {
        const order = keys.flatMap(key => {
          const reference = snapshot.threads.find(t => referenceKey(t) === key);
          return reference?.deviceId ? [{ deviceId: reference.deviceId, threadId: reference.threadId }] : [];
        });
        const next = await request<NavigationSnapshot>('relay/account/workbench/shortcuts', { method: 'PUT', body: JSON.stringify({ order }) });
        setSnapshot(next);
      } else {
        const next = reorder(loadLocal(storageKey));
        localStorage.setItem(storageKey, JSON.stringify(next));
        setSnapshot(next);
      }
    } catch (e) {
      setSnapshot(previous);
      setError(e instanceof Error ? e.message : translate("workbench.couldNotSaveShortcut"));
    }
  };
  const refs = hostScope ? snapshot.threads.filter(r => r.deviceId === null &&
    (r.threadId === detail?.thread.id || threads.some(thread => thread.id === r.threadId))) : [...snapshot.threads];
  if (detail && !refs.some((r) => referenceKey(r) === currentKey))
    refs.unshift({
      deviceId,
      threadId: detail.thread.id,
      title: detail.thread.title,
      workspaceLabel: detail.workspace.label,
      favorite: false,
      visitedAt: new Date().toISOString(),
    });
  const items: WorkbenchThread[] = refs.map((r) => {
    const local =
      r.deviceId === deviceId
        ? (r.threadId === detail?.thread.id ? detail.thread : threads.find((t) => t.id === r.threadId))
        : undefined;
    const activity = local ?? statuses[referenceKey(r)];
    return {
      key: referenceKey(r),
      title: local?.title ?? r.title,
      subtitle: [r.deviceName, r.workspaceLabel].filter(Boolean).join(' · '),
      href: threadHref(r.threadId, r.deviceId),
      favorite: r.favorite,
      ...(r.favorite && typeof r.favoriteOrder === 'number' ? { shortcutOrder: r.favoriteOrder } : {}),
      ...(activity?.parentThreadId ? {parentKey: `${r.deviceId ?? 'local'}:${activity.parentThreadId}`} : {}),
      ...(activity?.rootThreadId ? {rootKey: `${r.deviceId ?? 'local'}:${activity.rootThreadId}`} : {}),
      status: workbenchThreadStatus(activity, r.readCompletedAt),
      ...(local ? threadAgentField(local) : {}),
    };
  });
  const localNotifications: WorkbenchNotification[] = relay
    ? []
    : threads
        .filter((t) => t.lastTurnCompletedAt)
        .map((t) => ({
          id: `${t.id}:${t.lastTurnCompletedAt}`,
          title: `${t.title} ${t.status === 'failed' ? 'failed' : 'completed'}`,
          href: threadHref(t.id),
          occurredAt: t.lastTurnCompletedAt!,
        }))
        .sort((a, b) => b.occurredAt.localeCompare(a.occurredAt));
  const rawNotifications = [...(relay ? snapshot.notifications : localNotifications)]
    .sort((a, b) => Date.parse(b.occurredAt) - Date.parse(a.occurredAt))
    .slice(0, 10);
  const notificationKey = JSON.stringify(rawNotifications.map(n => [n.id, n.href, n.occurredAt]));
  useEffect(() => {
    if (!notificationsOpened) return;
    let alive = true;
    const controller = new AbortController();
    const pending = rawNotifications.filter(n => !notificationDetails[n.id]);
    void Promise.all(Array.from({ length: Math.min(3, pending.length) }, async () => {
      while (alive && pending.length) {
        const notification = pending.shift()!;
        // Fetch private previews through the viewer's authenticated device transport.
        const match = notification.href.match(/^\/devices\/([^/]+)\/threads\/([^/?#]+)$/);
        const local = notification.href.match(/^\/threads\/([^/?#]+)$/);
        const base = match ? `relay/devices/${match[1]}/api/threads/${match[2]}` : local && !relay ? `api/threads/${local[1]}` : null;
        if (!base) continue;
        try {
          const value = await request<ThreadDetailDto>(`${base}?view=summary&limit=10&compact=1${narrationQuery()}`, { signal: AbortSignal.any([controller.signal, AbortSignal.timeout(5000)]) });
          const eventTime = Date.parse(notification.occurredAt);
          const turn = value.turns.filter(t => t.completedAt && Math.abs(Date.parse(t.completedAt) - eventTime) < 60_000)
            .sort((a, b) => Math.abs(Date.parse(a.completedAt!) - eventTime) - Math.abs(Date.parse(b.completedAt!) - eventTime))[0];
          const text = [...(turn?.items ?? [])].reverse().find(item => item.kind === 'agentMessage' && item.text.trim())?.text.trim().replace(/\s+/g, ' ') ?? '';
          const summary = text ? Array.from(text).slice(0, 180).join('') + (Array.from(text).length > 180 ? '…' : '') : translate("workbench.openTheThreadToViewItsResponse");
          // The mark says how the turn ended, as in the sidebar.
          const status = turn?.status === 'failed' ? 'failed' as const : turn?.status === 'interrupted' ? 'interrupted' as const : 'unread' as const;
          if (alive) setNotificationDetails(previous => ({ ...previous, [notification.id]: { title: `${value.thread.title} · ${status === 'failed' ? translate("workbench.failed") : status === 'interrupted' ? translate("workbench.interrupted") : translate("workbench.completed")}`, summary, status } }));
        } catch { /* Leave metadata visible; retry unavailable previews when the bell opens again. */ }
      }
    }));
    return () => { alive = false; controller.abort(); };
  }, [notificationKey, notificationsOpened, relay]);
  // A notification from an agent thread is shown inside its conversation.
  const notifications = rawNotifications.map(n => {
    const match = n.href.match(/^\/devices\/([^/]+)\/threads\/([^/?#]+)$/) ?? n.href.match(/^\/threads\/([^/?#]+)$/);
    const [device, id] = match?.length === 3 ? [match[1]!, match[2]!] : [deviceId ?? 'local', match?.[1]];
    const pool = device === (deviceId ?? 'local') ? threads : deviceLists[device] ?? families[`${device}:${id}`] ?? [];
    const thread = pool.find(item => item.id === id) ?? Object.values(families).flat().find(item => item.id === id);
    const root = thread?.rootThreadId ? (pool.find(item => item.id === thread.rootThreadId) ?? Object.values(families).flat().find(item => item.id === thread.rootThreadId)) : undefined;
    return {
      ...n, ...notificationDetails[n.id],
      ...(thread ? { threadKey: `${device}:${thread.id}` } : {}),
      ...(root ? { rootKey: `${device}:${root.id}`, rootTitle: root.title, rootHref: threadHref(root.id, device === 'local' ? null : device) } : {}),
    };
  });
  // The tab bar lists the conversations started in the current workspace, each
  // with all its agent threads wherever they work. An agent thread belongs to its
  // conversation's workspace: one working here for a conversation elsewhere is
  // listed there, unless it is open, when it shows inside its conversation.
  // While a switched-to thread loads, its entry in the device list places the tabs.
  const open = tabContext.detail?.thread
    ?? (routeThreadId ? threads.find(thread => thread.id === routeThreadId) : undefined);
  const workspaceId = open?.workspaceId;
  const tabDevice = tabContext.deviceId ?? 'local';
  // Each listed thread says where it works: a label from the visits saved on this
  // device, then from the device itself, then the open thread's own.
  const workspaceLabels = new Map<string, string>();
  for (const r of snapshot.threads) {
    if ((r.deviceId ?? 'local') === tabDevice && r.workspaceId && r.workspaceLabel) workspaceLabels.set(r.workspaceId, r.workspaceLabel);
  }
  if (readLabels.device === tabDevice) for (const [id, label] of Object.entries(readLabels.labels)) workspaceLabels.set(id, label);
  if (tabContext.detail && workspaceId) workspaceLabels.set(workspaceId, tabContext.detail.workspace.label);
  const workspaceRoots = new Set(tabContext.threads
    .filter(thread => thread.workspaceId === workspaceId && !thread.rootThreadId)
    .map(thread => thread.id));
  const tabListed = tabContext.threads
      .filter(thread => workspaceRoots.has(thread.rootThreadId ?? thread.id) || thread.id === open?.id
        || (!!open?.rootThreadId && thread.id === open.rootThreadId));
  const openParent = open?.parentThreadId ? tabContext.threads.find(thread => thread.id === open.parentThreadId) : undefined;
  const unlabelled = [...new Set([...tabListed, ...(openParent ? [openParent] : [])].map(thread => thread.workspaceId))]
    .filter(id => id && !workspaceLabels.has(id) && !(readLabels.device === tabDevice && readLabels.tried.includes(id)))
    .sort().join(',');
  useEffect(() => {
    if (!unlabelled) return;
    const device = tabDevice;
    setReadLabels(previous => previous.device === device
      ? { ...previous, tried: [...previous.tried, ...unlabelled.split(',')] }
      : { device, labels: {}, tried: unlabelled.split(',') });
    // Shared and scoped readers may not list workspaces; their tabs keep blank places.
    void request<WorkspaceDto[]>(device === 'local' ? 'api/workspaces' : `relay/devices/${encodeURIComponent(device)}/api/workspaces`, { cache: 'no-store' })
      .then(list => {
        if (!mounted.current || !Array.isArray(list)) return;
        setReadLabels(previous => previous.device !== device ? previous
          : { ...previous, labels: { ...previous.labels, ...Object.fromEntries(list.map(workspace => [workspace.id, workspace.label])) } });
      })
      .catch(() => undefined);
  }, [unlabelled, tabDevice]);
  const orderScope = workspaceId ? `${tabDevice}:${workspaceId}` : '';
  const rank = new Map((workspaceOrders[orderScope] ?? []).map((key, index) => [key, index]));
  // Dragged conversations first in their order, the rest after them by age; an
  // agent thread goes with its root.
  const rankOf = (thread: ThreadDto) => rank.get(`${tabDevice}:${thread.rootThreadId ?? thread.id}`) ?? Number.MAX_SAFE_INTEGER;
  const workspaceThreads = tabListed
      .filter(thread => !localEdits.removed.includes(`${tabContext.deviceId ?? 'local'}:${thread.id}`))
      .slice()
      .sort((a, b) => rankOf(a) - rankOf(b) || (a.createdAt ?? '').localeCompare(b.createdAt ?? '') || a.id.localeCompare(b.id))
      .map(thread => {
        const key = `${tabContext.deviceId ?? 'local'}:${thread.id}`;
        const reference = snapshot.threads.find(r => referenceKey(r) === key);
        const renamed = localEdits.renamed[key];
        return {
          key, title: renamed?.from === thread.title ? renamed.to : thread.title, subtitle: workspaceLabels.get(thread.workspaceId) ?? '',
          href: threadHref(thread.id, tabContext.deviceId), favorite: reference?.favorite ?? false,
          ...(thread.parentThreadId ? {parentKey: `${tabContext.deviceId ?? 'local'}:${thread.parentThreadId}`} : {}),
          ...(thread.rootThreadId ? {rootKey: `${tabContext.deviceId ?? 'local'}:${thread.rootThreadId}`} : {}),
          status: workbenchThreadStatus(thread.id === tabContext.detail?.thread.id ? tabContext.detail.thread : thread, reference?.readCompletedAt),
          ...threadAgentField(thread),
        };
      });
  // Sidebar families are independent of the selected workspace. Current-device
  // list snapshots supply live updates; other devices use their scoped groups.
  const familyThreads = Object.entries(families).flatMap(([rootKey, family]) => {
    const familyDevice = rootKey.slice(0, rootKey.indexOf(':'));
    return family.map(thread => ({ deviceId: familyDevice, thread }));
  });
  familyThreads.push(...threads.map(thread => ({ deviceId: deviceId ?? 'local', thread })));
  const familyByKey = new Map(familyThreads.map(({ deviceId: familyDevice, thread }) => {
    const key = `${familyDevice}:${thread.id}`;
    const reference = snapshot.threads.find(r => referenceKey(r) === key);
    const activity = familyDevice === (deviceId ?? 'local') && thread.id === detail?.thread.id ? detail.thread : thread;
    // Its own workspace, also for an agent thread working away from its root.
    const rootReference = snapshot.threads.find(r => r.deviceId === familyDevice && r.threadId === (thread.rootThreadId ?? thread.id));
    const place = reference?.workspaceLabel
      ?? (familyDevice === tabDevice ? workspaceLabels.get(thread.workspaceId) : undefined)
      ?? snapshot.threads.find(r => (r.deviceId ?? 'local') === familyDevice && r.workspaceId === thread.workspaceId)?.workspaceLabel
      ?? (rootReference && (!rootReference.workspaceId || rootReference.workspaceId === thread.workspaceId) ? rootReference.workspaceLabel : undefined);
    return [key, {
      key, title: activity.title,
      subtitle: [reference?.deviceName, place].filter(Boolean).join(' · '),
      href: threadHref(thread.id, familyDevice === 'local' ? null : familyDevice),
      favorite: reference?.favorite ?? false,
      ...(thread.parentThreadId ? { parentKey: `${familyDevice}:${thread.parentThreadId}` } : {}),
      ...(thread.rootThreadId ? { rootKey: `${familyDevice}:${thread.rootThreadId}` } : {}),
      status: workbenchThreadStatus(statuses[key]?.status === 'unknown' && familyDevice !== (deviceId ?? 'local')
        ? { ...activity, status: 'unknown' } : activity, reference?.readCompletedAt),
      ...threadAgentField(thread),
    } satisfies WorkbenchThread] as const;
  }));
  // Running threads across the devices read above and this device's own list,
  // including ones never opened (automations, threads other agents started).
  const ownDevice = deviceId ?? 'local';
  const referenceOn = (device: string) => snapshot.threads.filter(r => (r.deviceId ?? 'local') === device);
  // Where a listed thread lives: its device's name and its workspace's label.
  const placeOf = (device: string, thread: ThreadDto) => {
    const references = referenceOn(device);
    const workspaceLabel = references.find(r => r.threadId === thread.id)?.workspaceLabel
      ?? references.find(r => r.workspaceId === thread.workspaceId)?.workspaceLabel
      ?? (device === tabDevice ? workspaceLabels.get(thread.workspaceId) : undefined);
    return [references.find(r => r.deviceName)?.deviceName, workspaceLabel];
  };
  const activeThreads: WorkbenchThread[] = [
    [ownDevice, threads] as const,
    ...Object.entries(deviceLists).filter(([device]) => device !== ownDevice),
  ].flatMap(([device, list]) => list.flatMap(thread => {
    const activity = device === ownDevice && thread.id === detail?.thread.id ? detail.thread : thread;
    if (workbenchThreadStatus(activity) !== 'running') return [];
    const key = `${device}:${thread.id}`;
    const reference = referenceOn(device).find(r => r.threadId === thread.id);
    // How long its turn has run says the most about a running thread.
    const startedAt = Date.parse(activity.lastTurnStartedAt ?? '');
    const minutes = Number.isFinite(startedAt) ? Math.max(0, Math.round((Date.now() - startedAt) / 60_000)) : null;
    const runningFor = minutes === null ? null : minutes < 1 ? translate('workbench.runningJustNow')
      : minutes < 60 ? translate('workbench.runningForMinutes', { count: minutes })
        : translate('workbench.runningForHours', { hours: Math.floor(minutes / 60), minutes: minutes % 60 });
    return [{
      key, title: activity.title,
      subtitle: [...placeOf(device, thread), runningFor].filter(Boolean).join(' · '),
      href: threadHref(thread.id, device === 'local' ? null : device),
      favorite: reference?.favorite ?? false,
      ...(thread.parentThreadId ? { parentKey: `${device}:${thread.parentThreadId}` } : {}),
      ...(thread.rootThreadId ? { rootKey: `${device}:${thread.rootThreadId}` } : {}),
      status: 'running',
      ...threadAgentField(thread),
    } satisfies WorkbenchThread];
  }));
  // Finished threads the reader opened before and has not read since; threads
  // never opened are not "unread", or every old workspace thread would be.
  activeThreads.push(...items
    .map(thread => ({ ...thread, ...familyByKey.get(thread.key) }))
    .filter(thread => thread.status === 'unread' && thread.key !== currentKey));
  // An agent thread is only shown inside its conversation, so its root comes along.
  const listedThreads = new Map<string, { device: string; thread: ThreadDto }>([[ownDevice, threads] as const, ...Object.entries(deviceLists)]
    .flatMap(([device, list]) => list.map(thread => [`${device}:${thread.id}`, { device, thread }] as const)));
  for (const thread of [...activeThreads]) {
    if (!thread.rootKey || activeThreads.some(active => active.key === thread.rootKey)) continue;
    const listed = listedThreads.get(thread.rootKey);
    const root = familyByKey.get(thread.rootKey) ?? (listed && {
      key: thread.rootKey, title: listed.thread.title,
      subtitle: placeOf(listed.device, listed.thread).filter(Boolean).join(' · '),
      href: threadHref(listed.thread.id, listed.device === 'local' ? null : listed.device),
      favorite: false,
      status: workbenchThreadStatus(listed.thread),
      ...threadAgentField(listed.thread),
    } satisfies WorkbenchThread);
    if (root) activeThreads.push(root);
  }
  const roots = new Set(items.map(thread => thread.rootKey ?? thread.key));
  const sidebarThreads = items.map(thread => ({...thread, ...familyByKey.get(thread.key)}));
  const seen = new Set(sidebarThreads.map(thread => thread.key));
  for (const thread of familyByKey.values()) {
    if (!seen.has(thread.key) && roots.has(thread.rootKey ?? thread.key)) {
      sidebarThreads.push(thread); seen.add(thread.key);
    }
  }
  return {
    navigationReady,
    /** The label of a workspace on the open thread's device, when known. */
    workspaceLabel: (id: string) => workspaceLabels.get(id),
    threads: sidebarThreads,
    activeThreads,
    workspacePath: tabContext.detail?.workspace.absPath ?? '',
    workspaceThreads,
    currentKey,
    favorite,
    favoriteBusy: busy,
    error,
    onToggleFavorite: () => void toggleFavorite(),
    onReorderShortcuts: (keys: string[]) => void reorderShortcuts(keys),
    onReorderWorkspaceThreads: orderScope ? (keys: string[]) => {
      setWorkspaceOrders(orders => {
        const next = { ...orders, [orderScope]: keys };
        try { localStorage.setItem(orderKey, JSON.stringify(next)); } catch { /* The order still holds for this visit. */ }
        return next;
      });
    } : undefined,
    onToggleThreadFavorite: (key: string) => toggleFavorite(key),
    onThreadRenamed: async (key: string, title: string) => {
      const listed = tabContext.threads.find(thread => `${tabContext.deviceId ?? 'local'}:${thread.id}` === key);
      if (listed) setLocalEdits(edits => ({ ...edits, renamed: { ...edits.renamed, [key]: { from: listed.title, to: title } } }));
      const target = snapshot.threads.find(t => referenceKey(t) === key);
      if (target) await save(undefined, false, { ...target, title });
    },
    onThreadRemoved: async (key: string) => {
      setLocalEdits(edits => ({ ...edits, removed: [...edits.removed, key] }));
      const target = snapshot.threads.find(t => referenceKey(t) === key);
      if (relay && !target) return;
      ++revision.current;
      if (relay && target) {
        setSnapshot(await request<NavigationSnapshot>('relay/account/workbench', { method: 'DELETE', body: JSON.stringify({ deviceId: target.deviceId, threadId: target.threadId }) }));
      } else {
        const next = loadLocal(storageKey);
        next.threads = next.threads.filter(t => referenceKey(t) !== key);
        localStorage.setItem(storageKey, JSON.stringify(next));
        setSnapshot(next);
      }
    },
    notifications,
    unreadCount: notifications.filter((n) => Date.parse(n.occurredAt) > readAt)
      .length,
    onReadNotifications: () => { setReadAt(Date.now()); setNotificationsOpened(value => value + 1); },
  };
}
