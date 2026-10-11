import { useCallback, useEffect, useRef } from 'react';

/** Refresh again on return when the data is at least this old. */
export const VISIBLE_RETURN_STALE_MS = 15_000;

const documentHidden = () =>
  typeof document !== 'undefined' && document.visibilityState === 'hidden';

/**
 * Runs `refresh` when something announces a change (`request`), on a slow
 * fallback timer, and on return to a hidden tab. Nothing runs while the tab is
 * hidden: requests made then are deferred to the return. Requests coalesce,
 * refreshes are at least `minIntervalMs` apart, and a request that arrives
 * during a refresh runs once after it.
 */
export function useEventDrivenRefresh({
  enabled,
  refresh,
  fallbackMs,
  coalesceMs = 300,
  minIntervalMs = 0,
}: {
  enabled: boolean;
  refresh: () => Promise<unknown>;
  fallbackMs: number;
  coalesceMs?: number;
  minIntervalMs?: number;
}) {
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;
  const stateRef = useRef({
    enabled,
    fallbackMs,
    coalesceMs,
    minIntervalMs,
    inFlight: false,
    pending: false,
    deferred: false,
    lastRunAt: 0,
    dueAt: 0,
    timer: null as ReturnType<typeof setTimeout> | null,
    fallbackTimer: null as ReturnType<typeof setTimeout> | null,
    run: () => {},
  });
  const state = stateRef.current;
  state.coalesceMs = coalesceMs;
  state.minIntervalMs = minIntervalMs;

  const schedule = useCallback((delayMs: number) => {
    const s = stateRef.current;
    if (!s.enabled) return;
    if (documentHidden()) {
      s.deferred = true;
      return;
    }
    const now = Date.now();
    const dueAt = Math.max(now + Math.max(0, delayMs), s.lastRunAt ? s.lastRunAt + s.minIntervalMs : 0);
    if (s.timer !== null && s.dueAt <= dueAt) return;
    if (s.timer !== null) clearTimeout(s.timer);
    s.dueAt = dueAt;
    s.timer = setTimeout(() => {
      s.timer = null;
      s.run();
    }, dueAt - now);
  }, []);

  const armFallback = useCallback(() => {
    const s = stateRef.current;
    if (s.fallbackTimer !== null) clearTimeout(s.fallbackTimer);
    s.fallbackTimer = null;
    if (!s.enabled) return;
    s.fallbackTimer = setTimeout(() => {
      s.fallbackTimer = null;
      s.run();
    }, s.fallbackMs);
  }, []);

  state.run = () => {
    const s = stateRef.current;
    if (!s.enabled) return;
    if (documentHidden()) {
      s.deferred = true;
      armFallback();
      return;
    }
    if (s.inFlight) {
      s.pending = true;
      return;
    }
    s.inFlight = true;
    s.deferred = false;
    void Promise.resolve()
      .then(() => refreshRef.current())
      .catch(() => {
        // The fallback timer or the next announced change retries.
      })
      .finally(() => {
        s.inFlight = false;
        s.lastRunAt = Date.now();
        armFallback();
        if (s.pending) {
          s.pending = false;
          schedule(s.coalesceMs);
        }
      });
  };

  useEffect(() => {
    const s = stateRef.current;
    s.enabled = enabled;
    s.fallbackMs = fallbackMs;
    if (!enabled) {
      if (s.timer !== null) clearTimeout(s.timer);
      if (s.fallbackTimer !== null) clearTimeout(s.fallbackTimer);
      s.timer = null;
      s.fallbackTimer = null;
      s.pending = false;
      s.deferred = false;
      return;
    }
    // Re-arm with the new interval, counting from the last refresh.
    if (s.fallbackTimer !== null) clearTimeout(s.fallbackTimer);
    const elapsed = s.lastRunAt ? Date.now() - s.lastRunAt : 0;
    s.fallbackTimer = setTimeout(() => {
      s.fallbackTimer = null;
      s.run();
    }, Math.max(0, fallbackMs - elapsed));
  }, [enabled, fallbackMs]);

  useEffect(() => {
    if (!enabled || typeof document === 'undefined') return;
    const onVisibility = () => {
      const s = stateRef.current;
      if (documentHidden()) return;
      if (s.deferred || Date.now() - s.lastRunAt >= Math.min(s.fallbackMs, VISIBLE_RETURN_STALE_MS)) {
        schedule(0);
      }
    };
    document.addEventListener('visibilitychange', onVisibility);
    return () => document.removeEventListener('visibilitychange', onVisibility);
  }, [enabled, schedule]);

  useEffect(() => () => {
    const s = stateRef.current;
    s.enabled = false;
    if (s.timer !== null) clearTimeout(s.timer);
    if (s.fallbackTimer !== null) clearTimeout(s.fallbackTimer);
  }, []);

  /** Ask for a refresh after `delayMs` (default: the coalescing window). */
  const request = useCallback((delayMs?: number) => {
    schedule(delayMs ?? stateRef.current.coalesceMs);
  }, [schedule]);
  /** Record a fetch done elsewhere so the fallback counts from it. */
  const markFresh = useCallback(() => {
    const s = stateRef.current;
    s.lastRunAt = Date.now();
    s.deferred = false;
    if (s.enabled) armFallback();
  }, [armFallback]);
  return { request, markFresh };
}
