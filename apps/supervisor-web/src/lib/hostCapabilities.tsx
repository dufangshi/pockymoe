import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';
import type { HostCapabilitiesDto, HostScopeDto } from '@pockymoe/shared';
import { translate } from '@pockymoe/thread-ui/i18n';
import { fetchRuntimeConfig } from './api';

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
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    setFailed(false);
    let active = true;
    void fetchRuntimeConfig().then(config => {
      if (active) setSnapshot({ scope, capabilities: config.hostCapabilities ?? standalone, hostScope: config.hostScope ?? null });
    }).catch(() => { if (active) setFailed(true); });
    return () => { active = false; };
  }, [scope, attempt]);
  // Wait for scope discovery before mounting routes or unrestricted controls.
  if (snapshot?.scope !== scope) return failed
    ? <p role="alert">{translate('workbench.statusUnavailable')} <button onClick={() => setAttempt(value => value + 1)}>{translate('files.retry')}</button></p>
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
