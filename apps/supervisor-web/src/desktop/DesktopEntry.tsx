import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useState, type ReactNode } from 'react';
import { ErrorMark } from '@pockymoe/thread-ui';
import { fetchRelaySession } from '../lib/api';
import { DesktopIndexStore, DesktopStoreContext } from './data';
import { desktopHost, type DesktopCapabilities } from './host';
import { TourProvider } from '../components/onboarding/TourProvider';
import { DesktopShell } from './DesktopShell';

export default function DesktopEntry({ children }: { children: ReactNode }) {
  useI18n();
  const [ready, setReady] = useState<{ store: DesktopIndexStore; capabilities: DesktopCapabilities } | null>(null);
  const [error, setError] = useState('');
  useEffect(() => {
    let cancelled = false; let store: DesktopIndexStore | undefined;
    void (async () => {
      const host = desktopHost(); if (!host) return;
      const [session, capabilities] = await Promise.all([fetchRelaySession(), host.capabilities()]);
      if (capabilities.surface !== 'desktop') return;
      if (!session.authenticated || !session.user) throw new Error(translate('desktopShell.signInRequired'));
      store = new DesktopIndexStore(session.user.id); await store.hydrate();
      if (cancelled) return;
      performance.mark('pockymoe-desktop-shell-mount'); setReady({ store, capabilities }); store.start();
    })().catch(error => { if (!cancelled) setError(error instanceof Error ? error.message : String(error)); });
    return () => { cancelled = true; store?.stop(); };
  }, []);
  if (!ready) return error ? <div className="p-6"><ErrorMark message={error} /></div> : null;
  return <DesktopStoreContext.Provider value={ready.store}><TourProvider><DesktopShell capabilities={ready.capabilities}>{children}</DesktopShell></TourProvider></DesktopStoreContext.Provider>;
}
