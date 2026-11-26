import { useEffect } from 'react';
import type { AgentBackendIdDto, AgentSubscriptionUsageDto } from '../../../../packages/shared/src/index';
import { fetchAgentSubscriptionUsage } from '../lib/api';
import { useScopedState } from './useScopedState';

const REFRESH_INTERVAL = 5 * 60_000;
const MAX_STALE_AGE = 30 * 60_000;
type CacheEntry = {
  usage: AgentSubscriptionUsageDto | null;
  refreshAt: number;
  failures: number;
  inFlight?: Promise<void> | undefined;
};
// Account usage belongs to a device/harness, not a turn or a thread. Reuse it
// when switching between parent and child threads (including older runtimes).
const cache = new Map<string, CacheEntry>();
// Mounted viewers of one device/harness account, woken by a harness report.
const listeners = new Map<string, Set<() => void>>();
const LIVE_REFRESH_GAP = 5_000;

function accountKey(deviceId: string | null, provider?: AgentBackendIdDto | string | null, agentId?: string | null) {
  // Match the Supervisor adapter key, including Claude reached through generic ACP.
  return JSON.stringify([deviceId ?? 'local', agentId ?? (provider === 'acp' ? 'codex' : provider) ?? null]);
}

/**
 * A running harness reported fresher allowance (`runtime.subscription.updated`).
 * The event carries no account data; mounted viewers re-read the owner-only
 * route now instead of at the next five-minute poll. Bursts collapse to one read.
 */
export function refreshSubscriptionUsage(deviceId: string | null, provider: string) {
  const key = accountKey(deviceId, provider === 'acp' ? 'codex' : provider);
  const entry = cache.get(key);
  if (!entry) return;
  if (!entry.inFlight) entry.refreshAt = Math.min(entry.refreshAt, Date.now() + LIVE_REFRESH_GAP);
  for (const wake of listeners.get(key) ?? []) wake();
}

function lastKnown(usage: AgentSubscriptionUsageDto | null) {
  if (!usage || !Number.isFinite(Date.parse(usage.observedAt)) ||
    Date.now() - Date.parse(usage.observedAt) >= MAX_STALE_AGE) return null;
  return { ...usage, stale: true };
}

function read(entry: CacheEntry) {
  if (entry.usage?.stale) entry.usage = lastKnown(entry.usage);
  return entry.usage;
}

function refresh(entry: CacheEntry, provider: AgentBackendIdDto, agentId?: string | null) {
  if (entry.inFlight) return entry.inFlight;
  if (Date.now() < entry.refreshAt) return Promise.resolve();
  const failed = () => {
    entry.usage = lastKnown(entry.usage);
    entry.failures++;
    entry.refreshAt = Date.now() + Math.min(REFRESH_INTERVAL * 2 ** Math.min(entry.failures - 1, 3), MAX_STALE_AGE);
  };
  entry.inFlight = Promise.resolve().then(async () => {
    try {
      const result = await fetchAgentSubscriptionUsage(provider, agentId);
      if (result.unavailable || (result.usage && result.usage.authKind !== 'subscription')) {
        // New runtimes explicitly distinguish missing auth from transient failures.
        entry.usage = null;
        entry.failures = 0;
        entry.refreshAt = Date.now() + REFRESH_INTERVAL;
      } else if (result.usage?.windows.length) {
        entry.usage = result.usage;
        if (result.usage.stale) failed();
        else {
          entry.failures = 0;
          entry.refreshAt = Date.now() + REFRESH_INTERVAL;
        }
      } else {
        // Old runtimes return usage:null on 429 too. Keep only a bounded,
        // explicitly stale last observation rather than making the UI flicker.
        failed();
      }
    } catch {
      failed();
    } finally {
      entry.inFlight = undefined;
    }
  });
  return entry.inFlight;
}

export function useSubscriptionUsage({
  deviceId, threadId, provider, agentId,
}: {
  deviceId: string | null;
  threadId: string;
  provider?: AgentBackendIdDto | undefined;
  agentId?: string | null | undefined;
}) {
  const key = accountKey(deviceId, provider, agentId);
  const [usage, setUsage] = useScopedState<AgentSubscriptionUsageDto | null>(key, null);
  useEffect(() => {
    if (!provider) return;
    // Remove expired entries to bound memory without discarding active requests.
    for (const [key, value] of cache) {
      if (!value.inFlight && value.refreshAt + MAX_STALE_AGE < Date.now()) cache.delete(key);
    }
    const entry = cache.get(key) ?? { usage: null, refreshAt: 0, failures: 0 };
    cache.set(key, entry);
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let resume: (() => void) | undefined;
    const poll = async () => {
      // A background tab (often a forgotten one) does not keep asking the device.
      if (typeof document !== 'undefined' && document.hidden) {
        resume = () => { document.removeEventListener('visibilitychange', resume!); resume = undefined; if (!cancelled) void poll(); };
        document.addEventListener('visibilitychange', resume);
        return;
      }
      setUsage(read(entry));
      await refresh(entry, provider, agentId);
      if (cancelled) return;
      setUsage(read(entry));
      const expiry = entry.usage?.stale ? Date.parse(entry.usage.observedAt) + MAX_STALE_AGE : Infinity;
      timer = setTimeout(() => { void poll(); }, Math.max(1, Math.min(entry.refreshAt, expiry) - Date.now()));
    };
    const wake = () => {
      if (cancelled || resume) return;
      clearTimeout(timer);
      timer = setTimeout(() => { void poll(); }, Math.max(1, entry.refreshAt - Date.now()));
    };
    const woken = listeners.get(key) ?? new Set();
    woken.add(wake);
    listeners.set(key, woken);
    void poll();
    return () => {
      cancelled = true; clearTimeout(timer); woken.delete(wake);
      if (!woken.size) listeners.delete(key);
      if (resume) document.removeEventListener('visibilitychange', resume);
    };
  }, [key, threadId, provider, agentId, setUsage]);
  return usage;
}
