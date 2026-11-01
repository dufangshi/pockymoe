import { useCallback, useEffect, useRef, useState } from 'react';
import { deviceRequest, request } from '../../lib/api';
import { useDesktopSelector } from '../data';
import { clearMetrics, readMetrics, warmMetrics, writeMetrics } from './cache';
import { isUnsupported, supportsStats, type DeviceMetrics, type StatsRange, type StatsSummary } from './model';

export function useDeviceMetrics(range: StatsRange) {
  const accountId = useDesktopSelector(index => index.accountId);
  const devices = useDesktopSelector(index => index.devices);
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
  const [metrics, setMetrics] = useState<Record<string, DeviceMetrics>>(() => warmMetrics(accountId, timeZone));
  const latest = useRef(metrics); latest.current = metrics;
  const [hydrated, setHydrated] = useState(false);
  const [showSkeleton, setShowSkeleton] = useState(false);
  const [refreshEpoch, setRefreshEpoch] = useState(0);
  const refreshMetrics = useCallback(() => setRefreshEpoch(epoch => epoch + 1), []);
  useEffect(() => {
    let stopped = false;
    void readMetrics(accountId, timeZone).then(value => { if (!stopped) { setMetrics(value); setHydrated(true); } });
    const logout = () => { stopped = true; latest.current = {}; setMetrics({}); void clearMetrics(accountId); };
    window.addEventListener('remote-codex:logout', logout);
    return () => { stopped = true; window.removeEventListener('remote-codex:logout', logout); };
  }, [accountId, timeZone]);
  useEffect(() => { const timer = setTimeout(() => setShowSkeleton(true), 200); return () => clearTimeout(timer); }, []);
  const signature = devices.map(d => `${d.id}:${d.connected}`).join('|');
  useEffect(() => {
    if (!hydrated) return;
    let stopped = false;
    let refreshing = false;
    const controllers = new Set<AbortController>();
    const patch = (id: string, value: Partial<DeviceMetrics>) => {
      if (stopped) return;
      const next = { ...latest.current, [id]: { summaries: {}, ...latest.current[id], ...value } };
      latest.current = next; setMetrics(next); void writeMetrics(accountId, timeZone, next);
    };
    const refresh = async () => {
      if (refreshing || document.visibilityState === 'hidden') return;
      refreshing = true;
      await Promise.allSettled(devices.filter(d => d.connected).map(async device => {
        const controller = new AbortController(); controllers.add(controller);
        const timeout = setTimeout(() => controller.abort(), 12_000);
        try {
          const start = performance.now();
          const health = await request<{ version?: string; capabilities?: { statsSummary?: boolean } }>(`/relay/devices/${encodeURIComponent(device.id)}/healthz`, { signal: controller.signal, cache: 'no-store' });
          patch(device.id, { latencyMs: Math.round(performance.now() - start), version: health.version, checkedAt: new Date().toISOString(), unavailable: false });
          // Read Settings' existing status, never start an update or an install here.
          const management = deviceRequest<{ runningVersion?: string; latestVersion?: string }>(device.id, '/api/management/supervisor', { signal: controller.signal }).then(status => {
            patch(device.id, { version: status.runningVersion ?? health.version, latestVersion: status.latestVersion });
          }).catch(() => undefined);
          if (!supportsStats(health)) patch(device.id, { partial: true });
          else {
            const ranges = [...new Set<StatsRange>(['today', '30d', range])];
            await Promise.all(ranges.map(async currentRange => {
              try {
                const summary = await deviceRequest<StatsSummary>(device.id, `/api/stats/summary?range=${currentRange}&tz=${encodeURIComponent(timeZone)}`, { signal: controller.signal, cache: 'no-store' });
                patch(device.id, { summaries: { ...latest.current[device.id]?.summaries, [currentRange]: summary }, partial: false });
              } catch (error) {
                if (isUnsupported(error)) patch(device.id, { partial: true });
                else if (!controller.signal.aborted) patch(device.id, { unavailable: true });
              }
            }));
          }
          await management;
        } catch { if (!stopped) patch(device.id, { unavailable: true, latencyMs: undefined }); }
        finally { clearTimeout(timeout); controllers.delete(controller); }
      }));
      refreshing = false;
    };
    void refresh();
    const timer = setInterval(() => { void refresh(); }, 30_000);
    const visible = () => { if (document.visibilityState === 'visible') void refresh(); };
    const logout = () => { stopped = true; clearInterval(timer); controllers.forEach(c => c.abort()); };
    document.addEventListener('visibilitychange', visible);
    window.addEventListener('remote-codex:logout', logout);
    return () => { logout(); document.removeEventListener('visibilitychange', visible); window.removeEventListener('remote-codex:logout', logout); };
    // Presence signature avoids cancelling probes for a name-only index update.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accountId, hydrated, range, refreshEpoch, signature, timeZone]);
  return { metrics, showSkeleton, hydrated, timeZone, refreshMetrics };
}
