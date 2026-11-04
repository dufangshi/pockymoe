import { useThreadReadState } from './useThreadReadState';
import { ThreadMapProvider } from '../components/ThreadMapHost';
import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { Navigate, useLocation, useNavigate } from 'react-router-dom';
import { PersistentWorkbenchNavigation } from '@pockymoe/thread-ui';
import { fetchRelaySession } from '../lib/api';
import { threadHref } from '../lib/relayRoutes';
import { appPathname } from '../lib/webBase';
import { useAppShellNav } from '../components/AppShellNavContext';
import { DesktopIndexStore, DesktopStoreContext } from './data';
import { DesktopSurfaceContext, hasWorkspaceContext } from './context';
import { ActivitySection } from './ActivitySection';
import { ShellRail } from './ShellRail';
import './desktop.css';
import './web-shell.css';

/** The account owns the index for the lifetime of this navigation shell. A
 * Devices route consumes it; leaving Devices must never stop sidebar updates. */
export default function WebEntry({ children }: { children: ReactNode }) {
  const [store, setStore] = useState<DesktopIndexStore | null>(null);
  const [signedOut, setSignedOut] = useState(false);
  const location = useLocation();
  useEffect(() => {
    let cancelled = false, created: DesktopIndexStore | undefined;
    void (async () => {
      const session = await fetchRelaySession();
      if (!session.authenticated || !session.user) throw new Error('signed out');
      created = new DesktopIndexStore(session.user.id); await created.hydrate();
      if (cancelled) return;
      setStore(created); created.start();
    })().catch(() => { if (!cancelled) setSignedOut(true); });
    return () => { cancelled = true; created?.stop(); };
  }, []);
  if (signedOut) return <Navigate to={'/relay-portal?returnTo=' + encodeURIComponent(location.pathname + location.search)} replace />;
  if (!store) return null;
  return <DesktopStoreContext.Provider value={store}><ThreadMapProvider><WebNavigation store={store}>{children}</WebNavigation></ThreadMapProvider></DesktopStoreContext.Provider>;
}
function WebNavigation({ children, store }: { children: ReactNode; store: DesktopIndexStore }) {
  const location = useLocation(), navigate = useNavigate(), shell = useAppShellNav();
  const open = useCallback((target: string) => {
    const thread = store.getSnapshot().threads.find(thread => thread.key === target);
    navigate(thread ? threadHref(thread.id, thread.deviceId) : appPathname(target));
  }, [navigate, store]);
  const openMap = useCallback(() => shell?.openThreadMap?.(), [shell?.openThreadMap]);
  const workspaceControlsEnabled = hasWorkspaceContext(location.pathname, location.search);
  const surface = useMemo(() => ({ multiWindow: false, routeReady() {}, workspaceControlsEnabled, openMap,
    revealInMap: openMap, railLeading: <ShellRail openMap={openMap} />,
    renderActivity: (navigation: import('./context').NavigationActions) => <ActivitySection navigation={navigation} onOpen={open} />,
  }), [openMap, open, workspaceControlsEnabled]);
  useThreadReadState(store, location.pathname);
  return <DesktopSurfaceContext.Provider value={surface}>
    <div className="web-navigation-shell thread-ui-shell thread-ui-viewport-constrained" data-navigation-shell data-theme-effective={shell?.effectiveTheme} data-theme-mode={shell?.themeMode}>
      <PersistentWorkbenchNavigation><div className="web-navigation-route">{children}</div></PersistentWorkbenchNavigation>
    </div>
  </DesktopSurfaceContext.Provider>;
}
