import { ProductHeader } from '../components/ProductHeader';
import { translate } from '@pockymoe/thread-ui/i18n';
import { useContext, useEffect, useState } from 'react';
import { Navigate, useLocation, useNavigate } from 'react-router-dom';
import { fetchRelaySession } from '../lib/api';
import { useAppShellNav } from '../components/AppShellNavContext';
import { DesktopIndexStore, DesktopStoreContext } from './data';
import { MobileIndexContext } from './data/context';
import { useHostSurface } from './surface';
import DevicesPage from './devices';
import { DesktopMapPage } from './MapPage';

/** Wide browsers consume the persistent shell's index and the client's frame. */
export default function WebDevicesRoute() {
  const store = useContext(DesktopStoreContext);
  const navigate = useNavigate();
  return store ? <DesktopMapPage visible mapVisible={false} revealKey={null} onOpen={target => {
    const thread = store.getSnapshot().threads.find(thread => thread.key === target);
    if (thread) navigate(`/devices/${encodeURIComponent(thread.deviceId)}/threads/${encodeURIComponent(thread.id)}`);
  }} /> : <StandaloneWebDevicesRoute />;
}
function StandaloneWebDevicesRoute() {
  const shell = useAppShellNav();
  const location = useLocation();
  // The mobile host already keeps the account's index; the page uses it.
  const mobileIndex = useContext(MobileIndexContext);
  const mobile = useHostSurface() === 'mobile';
  const [ownStore, setStore] = useState<DesktopIndexStore | null>(null);
  const store = mobile ? mobileIndex : ownStore;
  const [signedOut, setSignedOut] = useState(false);
  useEffect(() => {
    let cancelled = false, created: DesktopIndexStore | undefined;
    if (mobile) {
      // Signed out: the host's index never arrives; send the reader to sign in.
      void fetchRelaySession().then(session => { if (!cancelled && (!session.authenticated || !session.user)) setSignedOut(true); }).catch(() => { if (!cancelled) setSignedOut(true); });
      return () => { cancelled = true; };
    }
    void (async () => {
      const session = await fetchRelaySession();
      if (!session.authenticated || !session.user) throw new Error('signed out');
      created = new DesktopIndexStore(session.user.id); await created.hydrate();
      if (cancelled) return;
      setStore(created); created.start();
    })().catch(() => { if (!cancelled) setSignedOut(true); });
    return () => { cancelled = true; created?.stop(); };
  }, [mobile]);
  if (signedOut) return <Navigate to={'/relay-portal?returnTo=' + encodeURIComponent(location.pathname + location.search)} replace />;
  if (!store) return null;
  return <DesktopStoreContext.Provider value={store}>
    <div className="pd-web-page" data-theme-effective={shell?.effectiveTheme} data-theme-mode={shell?.themeMode}>
      <ProductHeader title={translate('devices.devices')} />
      <DevicesPage {...(shell?.openThreadMap ? { onRevealDevice: shell.openThreadMap } : {})} />
    </div>
  </DesktopStoreContext.Provider>;
}
