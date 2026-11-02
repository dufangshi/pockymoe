import { useEffect, useState } from 'react';
import { Link, Navigate } from 'react-router-dom';
import { ChevronLeft } from 'lucide-react';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { fetchRelaySession } from '../lib/api';
import { useAppShellNav } from '../components/AppShellNavContext';
import { DesktopIndexStore, DesktopStoreContext } from './data';
import DevicesPage from './devices';

/** The desktop's statistics page in a browser, on its own store while the page is open. */
export default function WebDevicesRoute() {
  const { t } = useI18n();
  const shell = useAppShellNav();
  const [store, setStore] = useState<DesktopIndexStore | null>(null);
  const [signedOut, setSignedOut] = useState(false);
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
  if (signedOut) return <Navigate to="/relay-devices" replace />;
  if (!store) return null;
  return <DesktopStoreContext.Provider value={store}>
    <div className="pd-web-page" data-theme-effective={shell?.effectiveTheme} data-theme-mode={shell?.themeMode}>
      <Link className="pd-web-back" to="/relay-devices"><ChevronLeft size={14} aria-hidden />{t('desktopDevices.backToDevices')}</Link>
      <DevicesPage {...(shell?.openThreadMap ? { onRevealDevice: shell.openThreadMap } : {})} />
    </div>
  </DesktopStoreContext.Provider>;
}
