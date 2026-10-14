import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';
import type { HostCapabilitiesDto } from '@pockymoe/shared';
import { fetchRuntimeConfig } from './api';

const standalone: HostCapabilitiesDto = {
  relay: true, accountBinding: true, devicePeer: true, previews: true,
};
const HostCapabilitiesContext = createContext(standalone);
export const useHostCapabilities = () => useContext(HostCapabilitiesContext);

/** Fetch after the existing access gate; old Supervisors retain their entry points. */
export function HostCapabilitiesProvider({ scope, children }: { scope: string; children: ReactNode }) {
  const [snapshot, setSnapshot] = useState<{ scope: string; capabilities: HostCapabilitiesDto } | null>(null);
  useEffect(() => {
    let active = true;
    void fetchRuntimeConfig().then(config => {
      if (active) setSnapshot({ scope, capabilities: config.hostCapabilities ?? standalone });
    }).catch(() => {});
    return () => { active = false; };
  }, [scope]);
  return <HostCapabilitiesContext.Provider value={snapshot?.scope === scope ? snapshot.capabilities : standalone}>
    {children}
  </HostCapabilitiesContext.Provider>;
}

export function HostFeatureGate({ feature, children }: { feature: keyof HostCapabilitiesDto; children: ReactNode }) {
  const capabilities = useHostCapabilities();
  return capabilities[feature] ? children : null;
}
