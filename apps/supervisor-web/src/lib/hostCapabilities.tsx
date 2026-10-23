import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import type { HostCapabilitiesDto, HostScopeDto } from '@pockymoe/shared';
import { translate } from '@pockymoe/thread-ui/i18n';
import { ErrorMark } from '@pockymoe/thread-ui';
import { fetchRuntimeConfig, relayModeActive } from './api';

const standalone: HostCapabilitiesDto = {
  relay: true, accountBinding: true, devicePeer: true, previews: true,
};
const HostScopeContext = createContext<HostScopeDto | null>(null);
export const useHostScope = () => useContext(HostScopeContext);

const HostCapabilitiesContext = createContext(standalone);
export const useHostCapabilities = () => useContext(HostCapabilitiesContext);

/** Fetch after the existing access gate; old Supervisors retain their entry points. */
type HostSnapshot = { scope: string; capabilities: HostCapabilitiesDto; hostScope: HostScopeDto | null };

export function HostCapabilitiesProvider({ scope, children }: { scope: string; children: ReactNode }) {
  const [snapshot, setSnapshot] = useState<HostSnapshot | null>(null);
  // What each device answered in this visit, shown at once when it is opened again.
  const known = useRef(new Map<string, HostSnapshot>());
  // Why the scope could not be read; null while it has not failed.
  const [failed, setFailed] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const relay = relayModeActive();
  // Relay pages outside a device describe no Supervisor; the relay's
  // compatibility route would answer for an arbitrary (often protected) device.
  const relayPage = relay && scope === 'local';
  useEffect(() => {
    if (relayPage) return;
    setFailed(null);
    let active = true;
    const settle = (next: HostSnapshot) => { known.current.set(next.scope, next); setSnapshot(next); };
    void fetchRuntimeConfig(relay ? scope : undefined).then(config => {
      if (active) settle({ scope, capabilities: config.hostCapabilities ?? standalone, hostScope: config.hostScope ?? null });
    }).catch((cause: unknown) => {
      if (!active) return;
      // A relay device that is offline or older keeps its usual pages; only a
      // local host must state its scope before routes mount.
      if (relay) settle({ scope, capabilities: standalone, hostScope: null });
      else setFailed(cause instanceof Error && cause.message ? cause.message : translate('workbench.statusUnavailable'));
    });
    return () => { active = false; };
  }, [scope, attempt, relay, relayPage]);
  // The same providers as on a device's pages, so moving between them keeps the app mounted.
  if (relayPage) return <HostScopeContext.Provider value={null}><HostCapabilitiesContext.Provider value={standalone}>{children}</HostCapabilitiesContext.Provider></HostScopeContext.Provider>;
  // Wait for scope discovery before mounting routes or unrestricted controls.
  // Nothing else can show: a short label, the reason behind a mark, and a visible Retry.
  // On the relay, moving to another device keeps the app mounted: the device's
  // last answer, or the previous device's until this one answers (returning
  // nothing here unmounted the whole app and blanked the page between devices).
  const shown = snapshot?.scope === scope ? snapshot : known.current.get(scope) ?? (relay ? snapshot : null);
  if (!shown) return failed !== null
    ? <p className="flex items-center gap-2">{translate('workbench.statusUnavailable')}<ErrorMark message={failed} /><button onClick={() => setAttempt(value => value + 1)}>{translate('files.retry')}</button></p>
    : null;
  return <HostScopeContext.Provider value={shown.hostScope}><HostCapabilitiesContext.Provider value={shown.hostScope ? { relay: false, accountBinding: false, devicePeer: false, previews: false } : shown.capabilities}>
    {children}
  </HostCapabilitiesContext.Provider></HostScopeContext.Provider>;
}

export function HostFeatureGate({ feature, children }: { feature: keyof HostCapabilitiesDto; children: ReactNode }) {
  const capabilities = useHostCapabilities();
  return capabilities[feature] ? children : null;
}

export function UnscopedOnly({ children }: { children: ReactNode }) {
  return useHostScope() ? null : children;
}
