import { useSyncExternalStore } from 'react';
import type { HostCapabilities, PockymoeHost } from './host';

type Surface = 'browser' | 'pending' | HostCapabilities['surface'];
const states = new WeakMap<PockymoeHost, { surface: Surface; capabilities?: HostCapabilities; listeners: Set<() => void> }>();
function state() {
  const host = window.pockymoeHost;
  if (!host) return null;
  let value = states.get(host);
  if (!value) {
    value = { surface: 'pending', listeners: new Set() }; states.set(host, value);
    const entry = value;
    void host.capabilities().then(capabilities => {
      entry.capabilities = capabilities; entry.surface = capabilities.surface;
    }).catch(() => { entry.surface = 'browser'; }).finally(() => entry.listeners.forEach(listener => listener()));
  }
  return value;
}
/** Host presence is transport availability, never evidence of desktop chrome. */
export function useHostSurface(): Surface {
  return useSyncExternalStore(listener => { const entry = state(); entry?.listeners.add(listener); return () => { entry?.listeners.delete(listener); }; }, () => state()?.surface ?? 'browser');
}
export function hostSurface(): Surface { return state()?.surface ?? 'browser'; }
export function mobileHost() {
  return state()?.surface === 'mobile' ? window.pockymoeHost : undefined;
}
export function playHaptic(kind: 'sent' | 'attention' | 'warning') {
  const entry = state();
  if (entry?.capabilities?.surface === 'mobile' && entry.capabilities.haptics) {
    try { mobileHost()?.haptics?.play(kind); } catch { /* Optional system feedback cannot fail an action. */ }
  }
}
