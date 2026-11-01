import { useCallback, useEffect, useRef, useState } from 'react';
import { request } from '../../lib/api';
import type { DesktopDevice } from '../data/types';
import { clearMetrics, readMetrics, warmMetrics, writeMetrics } from './cache';
import { dateKey, isUnsupported, supportsHourly, supportsStats, type DeviceMetrics, type StatsRange, type StatsSummary, type TurnOrigin } from './model';

export function useDeviceStats(accountId: string, devices: readonly DesktopDevice[], range: StatsRange, completedAt: string, refreshActivity: (deviceId: string) => Promise<void>, origin: TurnOrigin | '' = '') {
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
  const [metrics, setMetrics] = useState<Record<string, DeviceMetrics>>(() => warmMetrics(accountId, timeZone));
  const latest = useRef(metrics); latest.current = metrics;
  const [hydrated, setHydrated] = useState(false);
  const [skeleton, setSkeleton] = useState(false);
  const [epoch, setEpoch] = useState(0);
  const refresh = useCallback(() => setEpoch(value => value + 1), []);
  const refreshRef = useRef<() => void>(() => {});
  const activityRef = useRef(refreshActivity); activityRef.current = refreshActivity;
  const previousCompletion = useRef(completedAt);
  useEffect(() => {
    let disposed = false;
    void readMetrics(accountId, timeZone).then(value => { if (!disposed) { setMetrics(value); setHydrated(true); } });
    const logout = () => { disposed = true; latest.current = {}; setMetrics({}); void clearMetrics(accountId); };
    window.addEventListener('pockymoe-account-logout', logout);
    return () => { disposed = true; window.removeEventListener('pockymoe-account-logout', logout); };
  }, [accountId, timeZone]);
  useEffect(() => { const timer = setTimeout(() => setSkeleton(true), 200); return () => clearTimeout(timer); }, []);
  const signature = devices.map(d => `${d.id}:${d.connected}`).join('|');
  useEffect(() => {
    if (!hydrated) return;
    let stopped = false;
    const pending = new Set<string>();
    const health = new Map<string, { capabilities?: { statsSummary?: boolean } }>();
    const controllers = new Set<AbortController>();
    const patch = (id: string, value: Partial<DeviceMetrics>) => {
      if (stopped) return;
      const next = { ...latest.current, [id]: { summaries: {}, ...latest.current[id], ...value } };
      latest.current = next; setMetrics(next); void writeMetrics(accountId, timeZone, next);
    };
    const read = <T,>(id: string, path: string, signal: AbortSignal) => request<T>(`relay/devices/${encodeURIComponent(id)}/${path}`, { signal, cache: 'no-store' });
    const probe = async (device: DesktopDevice, stats: boolean) => {
      const key = `${device.id}:${stats}`;
      if (stopped || document.hidden || !device.connected || pending.has(key)) return;
      pending.add(key);
      const controller = new AbortController(); controllers.add(controller);
      const timeout = setTimeout(() => controller.abort(), 12_000);
      try {
        let capability = health.get(device.id);
        if (!stats || !capability) {
          const start = performance.now();
          const result = await read<{ version?: string; capabilities?: { statsSummary?: boolean } }>(device.id, 'healthz', controller.signal);
          capability = result; health.set(device.id, result);
          patch(device.id, { latencyMs: Math.round(performance.now() - start), ...(result.version ? { version: result.version } : {}), checkedAt: new Date().toISOString(), unavailable: false });
        }
        if (!stats) return;
        const management = read<{ runningVersion?: string; latestVersion?: string }>(device.id, 'api/management/supervisor', controller.signal).then(status => {
          patch(device.id, { ...(status.runningVersion ? { version: status.runningVersion } : {}), ...(status.latestVersion ? { latestVersion: status.latestVersion } : {}) });
        }).catch(() => undefined);
        let unsupported = !supportsStats(capability!);
        if (!unsupported) {
          await Promise.all([...new Set<StatsRange>(['today', '30d', range])].map(async selected => {
            try {
              const summary = await read<StatsSummary>(device.id, `api/stats/summary?range=${selected}&tz=${encodeURIComponent(timeZone)}`, controller.signal);
              patch(device.id, { summaries: { ...latest.current[device.id]?.summaries, [selected]: summary }, partial: false, unavailable: false, ...(selected === 'all' ? { allUnsupported: false } : {}) });
            } catch (error) {
              if (selected === 'all' && typeof error === 'object' && error !== null && 'statusCode' in error && [400, 404].includes(Number(error.statusCode))) { patch(device.id, { allUnsupported: true }); }
              else if (isUnsupported(error)) unsupported = true; else throw error;
            }
          }));
        }
        if (unsupported) {
          patch(device.id, { partial: true });
        }
        const base = latest.current[device.id]?.summaries[range];
        if (!unsupported && origin && Array.isArray(base?.byOrigin)) {
          try {
            const summary = await read<StatsSummary>(device.id, `api/stats/summary?range=${range}&tz=${encodeURIComponent(timeZone)}&origin=${origin}`, controller.signal);
            // Older endpoints can ignore unknown query parameters. Never cache
            // their unfiltered totals as a selected origin's usage.
            if (summary.origin === origin) patch(device.id, { originSummaries: { ...latest.current[device.id]?.originSummaries, [origin]: { ...latest.current[device.id]?.originSummaries?.[origin], [range]: summary } } });
          } catch { /* Keep the previous selected-origin snapshot in place. */ }
        }
        await management;
      } catch { if (!stopped) patch(device.id, { unavailable: true, latencyMs: undefined }); }
      finally { clearTimeout(timeout); controllers.delete(controller); pending.delete(key); }
    };
    const all = async () => {
      await Promise.allSettled(devices.filter(d => d.connected).map(d => probe(d, true)));
      if (stopped || document.hidden) return;
      const today = dateKey(new Date(), timeZone);
      const runtimeFallback = range === 'today' && devices.some(d => {
        const summary = latest.current[d.id]?.summaries.today;
        return !supportsHourly(summary) || summary.daily[0]?.date !== today;
      });
      await Promise.allSettled(devices.filter(d => d.connected && (runtimeFallback || latest.current[d.id]?.partial || range === 'all' && latest.current[d.id]?.allUnsupported)).map(d => activityRef.current(d.id)));
    };
    refreshRef.current = all;
    void all();
    const statsTimer = setInterval(all, 60_000);
    const latencyTimer = setInterval(() => { devices.forEach(d => { void probe(d, false); }); }, 30_000);
    const visible = () => { if (!document.hidden) all(); };
    const stop = () => { stopped = true; clearInterval(statsTimer); clearInterval(latencyTimer); controllers.forEach(c => c.abort()); };
    document.addEventListener('visibilitychange', visible); window.addEventListener('pockymoe-account-logout', stop);
    return () => { stop(); refreshRef.current = () => {}; document.removeEventListener('visibilitychange', visible); window.removeEventListener('pockymoe-account-logout', stop); };
    // Only identity/presence changes should restart transport, never a name-only update.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accountId, epoch, hydrated, origin, range, signature, timeZone]);
  useEffect(() => {
    if (previousCompletion.current === completedAt) return;
    previousCompletion.current = completedAt;
    const timer = setTimeout(() => refreshRef.current(), 2000);
    return () => clearTimeout(timer);
  }, [completedAt]);
  return { metrics, hydrated, skeleton, timeZone, refresh };
}
