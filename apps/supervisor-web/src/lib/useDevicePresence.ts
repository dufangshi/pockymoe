import { useEffect, useState } from 'react';
import type { RelayPortalSummaryDto } from '@pockymoe/shared';
import { request } from './api';

const PROBE_INTERVAL_MS = 3000;
const LIVE_FOR_MS = 5000;

// Keep the relay's socket inventory separate from observed reachability. A stale
// portal refresh must neither renew a lease nor stop probing an unreachable Mac.
export function useDevicePresence(raw: RelayPortalSummaryDto | null) {
  const targets = new Map<string, string>();
  for (const device of raw?.devices ?? []) {
    if (device.connected) targets.set(device.id, '');
  }
  for (const entry of [
    ...(raw?.sharedDevicesWithMe ?? []),
    ...(raw?.sharedThreadsWithMe ?? []),
    ...(raw?.sharedWithMe ?? []),
    ...(raw?.sharedByMe ?? []),
    ...(raw?.grantsByMe ?? []),
  ]) {
    if (!entry.deviceConnected || targets.has(entry.deviceId)) continue;
    const query = new URLSearchParams();
    if (entry.threadId) query.set('threadId', entry.threadId);
    else if (entry.workspaceId) query.set('workspaceId', entry.workspaceId);
    targets.set(entry.deviceId, query.toString());
  }
  const targetKey = JSON.stringify([...targets].sort(([a], [b]) => a.localeCompare(b)));
  const [online, setOnline] = useState<ReadonlySet<string>>(new Set());

  useEffect(() => {
    const entries = (JSON.parse(targetKey) as [string, string][]).map(([id, query]) => ({
      id, query, lastReply: 0, nextProbe: 0, controller: null as AbortController | null,
    }));
    let stopped = false;
    function publish() {
      const now = Date.now();
      const next = new Set(entries.filter(e => e.lastReply > 0 && now - e.lastReply < LIVE_FOR_MS).map(e => e.id));
      setOnline(previous => previous.size === next.size && [...next].every(id => previous.has(id)) ? previous : next);
    }
    function tick() {
      if (stopped || document.visibilityState === 'hidden') return;
      const now = Date.now();
      for (const entry of entries) {
        if (entry.controller || now < entry.nextProbe) continue;
        const controller = new AbortController();
        entry.controller = controller;
        entry.nextProbe = now + PROBE_INTERVAL_MS;
        const timeout = window.setTimeout(() => controller.abort(), LIVE_FOR_MS);
        void request<{ connected: boolean }>(
          `relay/devices/${encodeURIComponent(entry.id)}/presence?${entry.query}`,
          { signal: controller.signal, cache: 'no-store' },
        ).then(result => {
          // A delayed response cannot extend reachability past the lifetime of
          // its probe. The device may have slept while that reply was in flight.
          if (!stopped && !controller.signal.aborted) entry.lastReply = result.connected ? now : 0;
        }).catch(() => {
          if (!stopped && entry.controller === controller) entry.lastReply = 0;
        }).finally(() => {
          window.clearTimeout(timeout);
          if (entry.controller === controller) entry.controller = null;
          if (!stopped) publish();
        });
      }
      publish();
    }
    function visibilityChanged() {
      for (const entry of entries) {
        entry.controller?.abort();
        entry.controller = null;
        entry.lastReply = 0;
        entry.nextProbe = 0;
      }
      publish();
      tick();
    }
    tick();
    const timer = window.setInterval(tick, 250);
    document.addEventListener('visibilitychange', visibilityChanged);
    return () => {
      stopped = true;
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', visibilityChanged);
      for (const entry of entries) entry.controller?.abort();
    };
  }, [targetKey]);

  if (!raw) return null;
  const shared = <T extends { deviceId: string; deviceConnected?: boolean }>(entries: T[]) =>
    entries.map(entry => ({ ...entry, deviceConnected: Boolean(entry.deviceConnected && online.has(entry.deviceId)) }));
  return {
    ...raw,
    devices: raw.devices.map(device => ({ ...device, connected: device.connected && online.has(device.id) })),
    sharedWithMe: shared(raw.sharedWithMe),
    sharedByMe: shared(raw.sharedByMe),
    sharedDevicesWithMe: shared(raw.sharedDevicesWithMe ?? []),
    sharedThreadsWithMe: shared(raw.sharedThreadsWithMe ?? []),
    grantsByMe: shared(raw.grantsByMe ?? []),
  };
}
