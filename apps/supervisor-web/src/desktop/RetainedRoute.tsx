import { startTransition, useContext, useLayoutEffect, useRef, useState } from 'react';
import { UNSAFE_LocationContext, useLocation, useOutlet } from 'react-router-dom';
import { HostCapabilitiesProvider, useHostScope } from '../lib/hostCapabilities';
import { TourProvider } from '../components/onboarding/TourProvider';
import { HostScopeRouteGate } from '../components/HostScopeRouteGate';
import type { ReactNode } from 'react';
function ScopedTourProvider({ children }: { children: ReactNode }) { return useHostScope() ? children : <TourProvider>{children}</TourProvider>; }
import { AppShellSettingsDialog } from '../components/AppShellSettingsDialog';
import { prepareDesktopThread } from '../pages/ThreadDetailPage';
import { relayDeviceIdFromPath, threadIdFromPath } from '../lib/relayRoutes';
import { RetainedRouteActiveContext, useDesktopSurface } from './context';

/** Freeze the actual outlet AND its location, rather than an <Outlet /> element
 * that would keep reading the router's new child. Devices and Map are overlays;
 * they never replace the mounted thread, its scroll DOM or decryption state. */
export function RetainedDesktopRoute() {
  const location = useLocation();
  const locationContext = useContext(UNSAFE_LocationContext);
  const outlet = useOutlet();
  const overlay = location.pathname === '/' || location.pathname.startsWith('/desktop/devices');
  const snapshot = { outlet, locationContext, path: location.pathname, scope: relayDeviceIdFromPath(location.pathname) ?? 'local' };
  const [shown, setShown] = useState(() => overlay ? null : snapshot);
  const current = useRef(snapshot); current.current = snapshot;
  useLayoutEffect(() => {
    if (overlay) return;
    const next = current.current;
    let active = true;
    const commit = () => { if (active) startTransition(() => setShown(next)); };
    const thread = threadIdFromPath(next.path);
    if (thread && !['new', 'import'].includes(thread)) {
      // The previous page stays visible while the real summary loads. Errors
      // still enter the ordinary thread renderer and its retry/error state.
      void prepareDesktopThread(thread, next.scope === 'local' ? null : next.scope).then(commit, commit);
    } else startTransition(() => setShown(next));
    return () => { active = false; };
  }, [location.key, location.pathname, overlay]);
  return <>{shown ? <RetainedRouteActiveContext.Provider value={!overlay && location.pathname === shown.path}><UNSAFE_LocationContext.Provider value={shown.locationContext}>
    <HostCapabilitiesProvider scope={shown.scope}><ScopedTourProvider>
      <RoutePaintReady path={shown.path} visit={shown}><HostScopeRouteGate>{shown.outlet}</HostScopeRouteGate></RoutePaintReady>
    </ScopedTourProvider></HostCapabilitiesProvider>
  </UNSAFE_LocationContext.Provider></RetainedRouteActiveContext.Provider> : null}<AppShellSettingsDialog /></>;
}

/** Scope discovery is part of readiness too. Keep the previous page until this
 * child can mount, rather than hiding it while HostCapabilitiesProvider is null. */
function RoutePaintReady({ path, visit, children }: { path: string; visit: object; children: ReactNode }) {
  const ready = useDesktopSurface()?.routeReady;
  useLayoutEffect(() => { ready?.(path); }, [path, visit, ready]);
  return children;
}
