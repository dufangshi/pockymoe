import { useCallback } from 'react';

import type { ThreadDto } from '@pockymoe/shared';
import type { Dispatch, SetStateAction } from 'react';
import { fetchThreads, fetchWorkspaceThreads } from '../lib/api';
import { useEventDrivenRefresh } from '../lib/useEventDrivenRefresh';

/** Fallback while the Supervisor socket announces thread changes. */
export const THREAD_LIST_POLL_INTERVAL_MS = 60_000;
/** Fallback while no socket announces changes. */
export const THREAD_LIST_OFFLINE_POLL_INTERVAL_MS = 10_000;

function stableStringify(value: unknown) {
  return JSON.stringify(value);
}

function areThreadDtosEquivalent(left: ThreadDto, right: ThreadDto) {
  return stableStringify(left) === stableStringify(right);
}

export function mergeThreadListSnapshot(
  current: ThreadDto[],
  next: ThreadDto[],
) {
  const currentById = new Map(current.map((thread) => [thread.id, thread]));
  let changed = current.length !== next.length;
  const merged = next.map((nextThread, index) => {
    const currentThread = currentById.get(nextThread.id);
    if (
      currentThread &&
      areThreadDtosEquivalent(currentThread, nextThread)
    ) {
      if (current[index]?.id !== nextThread.id) {
        changed = true;
      }
      return currentThread;
    }

    changed = true;
    return nextThread;
  });

  return changed ? merged : current;
}

/**
 * Keeps a thread list current. Callers announce changes with `refresh()` (for
 * example on thread WebSocket events); otherwise the list is re-read on a slow
 * fallback (`intervalMs`, or `offlineIntervalMs` while `live` is false), never
 * while the tab is hidden, and right away on return to the tab.
 */
export function useThreadListPolling(input: {
  enabled: boolean;
  setThreads: Dispatch<SetStateAction<ThreadDto[]>>;
  intervalMs?: number;
  offlineIntervalMs?: number;
  /** A socket is connected and announces thread changes. */
  live?: boolean;
  includeAgentThreads?: boolean;
  /** A workspace share reads only its workspace's threads. */
  sharedWorkspaceId?: string | null;
}) {
  const {
    enabled,
    setThreads,
    intervalMs = THREAD_LIST_POLL_INTERVAL_MS,
    offlineIntervalMs = THREAD_LIST_OFFLINE_POLL_INTERVAL_MS,
    live = true,
    includeAgentThreads = false,
    sharedWorkspaceId = null,
  } = input;

  const load = useCallback(async () => {
    try {
      const nextThreads = sharedWorkspaceId
        ? await fetchWorkspaceThreads(sharedWorkspaceId, includeAgentThreads)
        : await fetchThreads(includeAgentThreads);
      setThreads((current) => mergeThreadListSnapshot(current, nextThreads));
    } catch {
      // Keep the existing room list stable; the next refresh can recover.
    }
  }, [setThreads, includeAgentThreads, sharedWorkspaceId]);

  const { request, markFresh } = useEventDrivenRefresh({
    enabled,
    refresh: load,
    fallbackMs: live ? intervalMs : Math.min(intervalMs, offlineIntervalMs),
    coalesceMs: 400,
    // Busy devices announce many turns; re-read at most every 2 s.
    minIntervalMs: 2_000,
  });
  return { refresh: request, markFresh };
}
