import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';
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
export function HostCapabilitiesProvider({ scope, children }: { scope: string; children: ReactNode }) {
  const [snapshot, setSnapshot] = useState<{ scope: string; capabilities: HostCapabilitiesDto; hostScope: HostScopeDto | null } | null>(null);
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
    void fetchRuntimeConfig(relay ? scope : undefined).then(config => {
      if (active) setSnapshot({ scope, capabilities: config.hostCapabilities ?? standalone, hostScope: config.hostScope ?? null });
    }).catch((cause: unknown) => {
      if (!active) return;
      // A relay device that is offline or older keeps its usual pages; only a
      // local host must state its scope before routes mount.
      if (relay) setSnapshot({ scope, capabilities: standalone, hostScope: null });
      else setFailed(cause instanceof Error && cause.message ? cause.message : translate('workbench.statusUnavailable'));
    });
    return () => { active = false; };
  }, [scope, attempt, relay, relayPage]);
  if (relayPage) return <HostCapabilitiesContext.Provider value={standalone}>{children}</HostCapabilitiesContext.Provider>;
  // Wait for scope discovery before mounting routes or unrestricted controls.
  // Nothing else can show: a short label, the reason behind a mark, and a visible Retry.
  if (snapshot?.scope !== scope) return failed !== null
    ? <p className="flex items-center gap-2">{translate('workbench.statusUnavailable')}<ErrorMark message={failed} /><button onClick={() => setAttempt(value => value + 1)}>{translate('files.retry')}</button></p>
    : null;
  return <HostScopeContext.Provider value={snapshot.hostScope}><HostCapabilitiesContext.Provider value={snapshot.hostScope ? { relay: false, accountBinding: false, devicePeer: false, previews: false } : snapshot.capabilities}>
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
