import { useHostCapabilities } from '../lib/hostCapabilities';
import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  RelayDeviceDto,
  RelayPortalSummaryDto,
  ThreadDto,
  WorkspaceDto,
} from '@pockymoe/shared';
import type { WorkbenchThread } from '@pockymoe/thread-ui';
import { request } from '../lib/api';

export interface SplitThreadSelection {
  deviceId: string | null;
  workspaceId: string;
  threadId: string;
  title: string;
}
export type SplitNavigationThread = Pick<
  WorkbenchThread,
  'key' | 'favorite'
> & { visitedAt?: string };
export interface CatalogResource<T> {
  items: T[];
  loading: boolean;
  loaded: boolean;
  error: string | null;
}
const empty = <T>(): CatalogResource<T> => ({
  items: [],
  loading: false,
  loaded: false,
  error: null,
});
const deviceKey = (id: string | null) => id ?? 'local';
export const splitThreadKey = (deviceId: string | null, threadId: string) =>
  `${deviceKey(deviceId)}:${threadId}`;

/** Favorites and visits stay inside their device/workspace branch. */
export function rankSplitThreads(
  threads: ThreadDto[],
  deviceId: string | null,
  navigation: SplitNavigationThread[] = [],
) {
  const visits = new Map(
    navigation.map((entry, index) => [entry.key, { ...entry, index }]),
  );
  return [...threads].sort((a, b) => {
    const av = visits.get(splitThreadKey(deviceId, a.id));
    const bv = visits.get(splitThreadKey(deviceId, b.id));
    return (
      Number(Boolean(bv?.favorite || b.isPinned)) -
        Number(Boolean(av?.favorite || a.isPinned)) ||
      Number(Boolean(bv)) - Number(Boolean(av)) ||
      (av && bv
        ? (bv.visitedAt ?? '').localeCompare(av.visitedAt ?? '') ||
          av.index - bv.index
        : 0) ||
      b.updatedAt.localeCompare(a.updatedAt) ||
      a.title.localeCompare(b.title)
    );
  });
}
export function rankSplitDevices(
  devices: RelayDeviceDto[],
  navigation: SplitNavigationThread[] = [],
) {
  const priority = (device: RelayDeviceDto) => {
    const entries = navigation
      .map((entry, index) => ({ ...entry, index }))
      .filter((entry) => entry.key.startsWith(`${device.id}:`));
    return {
      favorite: entries.some((entry) => entry.favorite),
      recent: Math.min(...entries.map((entry) => entry.index)),
    };
  };
  const priorities = new Map(
    devices.map((device) => [device.id, priority(device)]),
  );
  return [...devices].sort((a, b) => {
    const av = priorities.get(a.id)!,
      bv = priorities.get(b.id)!;
    return (
      Number(bv.favorite) - Number(av.favorite) ||
      av.recent - bv.recent ||
      a.name.localeCompare(b.name)
    );
  });
}
export function rankSplitWorkspaces(workspaces: WorkspaceDto[]) {
  return [...workspaces].sort(
    (a, b) =>
      Number(Boolean(b.isFavorite)) - Number(Boolean(a.isFavorite)) ||
      (b.lastOpenedAt ?? '').localeCompare(a.lastOpenedAt ?? '') ||
      a.label.localeCompare(b.label),
  );
}

/** Explicit transports never mutate the browser's selected device or route. */
export function useWorkbenchSplitCatalog(
  open: boolean,
  deviceId: string | null,
  workspaceId: string,
) {
  const { devicePeer } = useHostCapabilities();
  const scope = JSON.stringify([deviceId, workspaceId]);
  const owner = useRef({
    scope,
    workspaces: new Map<string, CatalogResource<WorkspaceDto>>(),
    threads: new Map<string, CatalogResource<ThreadDto>>(),
    devices: empty<RelayDeviceDto>(),
  });
  if (owner.current.scope !== scope)
    owner.current = {
      scope,
      workspaces: new Map(),
      threads: new Map(),
      devices: empty(),
    };
  const current = owner.current;
  const [, redraw] = useState(0);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const publish = useCallback(() => {
    if (mounted.current && owner.current === current) redraw((n) => n + 1);
  }, [current]);
  const load = useCallback(
    async <T>(
      map: Map<string, CatalogResource<T>>,
      id: string | null,
      path: string,
      retry = false,
    ) => {
      const key = deviceKey(id);
      const previous = map.get(key) ?? empty<T>();
      if (
        previous.loading ||
        (previous.loaded && !retry) ||
        (previous.error && !retry)
      )
        return;
      map.set(key, { ...previous, loading: true, error: null });
      publish();
      const base = id ? `/relay/devices/${encodeURIComponent(id)}` : '';
      try {
        const items = await request<T[]>(`${base}${path}`, {
          signal: AbortSignal.timeout(15_000),
          cache: 'no-store',
        });
        if (!Array.isArray(items)) throw new Error('Invalid catalog response');
        map.set(key, { items, loading: false, loaded: true, error: null });
      } catch (error) {
        map.set(key, {
          ...previous,
          loading: false,
          error: error instanceof Error ? error.message : String(error),
        });
      }
      publish();
    },
    [current, publish],
  );
  const loadWorkspaces = useCallback(
    (id: string | null, retry = false) =>
      load(current.workspaces, id, '/api/workspaces', retry),
    [current, load],
  );
  const loadThreads = useCallback(
    (id: string | null, retry = false) =>
      load(current.threads, id, '/api/threads?includeAgentThreads=true', retry),
    [current, load],
  );
  const loadDevices = useCallback(
    async (retry = false) => {
      if (
        !devicePeer || !deviceId ||
        current.devices.loading ||
        (current.devices.loaded && !retry) ||
        (current.devices.error && !retry)
      )
        return;
      current.devices = { ...current.devices, loading: true, error: null };
      publish();
      try {
        const portal = await request<RelayPortalSummaryDto>('/relay/portal', {
          signal: AbortSignal.timeout(15_000),
          cache: 'no-store',
        });
        // Only the user's own devices; shared access keeps its existing server checks.
        current.devices = {
          items: portal.devices.filter((device) => device.id !== deviceId),
          loading: false,
          loaded: true,
          error: null,
        };
      } catch (error) {
        current.devices = {
          ...current.devices,
          loading: false,
          error: error instanceof Error ? error.message : String(error),
        };
      }
      publish();
    },
    [current, deviceId, devicePeer, publish],
  );
  useEffect(() => {
    if (!open) return;
    void loadWorkspaces(deviceId);
    void loadThreads(deviceId);
    void loadDevices();
  }, [open, deviceId, loadWorkspaces, loadThreads, loadDevices]);
  return {
    devices: current.devices,
    workspaces: (id: string | null) =>
      current.workspaces.get(deviceKey(id)) ?? empty<WorkspaceDto>(),
    threads: (id: string | null) =>
      current.threads.get(deviceKey(id)) ?? empty<ThreadDto>(),
    loadWorkspaces,
    loadThreads,
    loadDevices,
  };
}
