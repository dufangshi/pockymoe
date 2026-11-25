import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useState, type ReactNode } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { ErrorMark, setHostPlatform } from '@pockymoe/thread-ui';
import { fetchRelaySession } from '../lib/api';
import { DesktopIndexStore, DesktopStoreContext } from './data';
import { desktopHost, type DesktopCapabilities } from './host';
import { TourProvider } from '../components/onboarding/TourProvider';
import { DesktopShell } from './DesktopShell';

export default function DesktopEntry({ children }: { children: ReactNode }) {
  useI18n();
  const [ready, setReady] = useState<{ store: DesktopIndexStore; capabilities: DesktopCapabilities } | null>(null);
  const [error, setError] = useState<{ signIn: boolean; message: string } | null>(null);
  const [attempt, setAttempt] = useState(0);
  const navigate = useNavigate(), location = useLocation();
  useEffect(() => {
    let cancelled = false; let store: DesktopIndexStore | undefined;
    void (async () => {
      const host = desktopHost(); if (!host) return;
      const [session, capabilities] = await Promise.all([fetchRelaySession(), host.capabilities()]);
      if (capabilities.surface !== 'desktop') return;
      // The client's operating system decides which key is Mod (§19).
      setHostPlatform(capabilities.platform);
      if (!session.authenticated || !session.user) { if (!cancelled) setError({ signIn: true, message: translate('desktopShell.signInRequired') }); return; }
      store = new DesktopIndexStore(session.user.id); await store.hydrate();
      if (cancelled) return;
      performance.mark('pockymoe-desktop-shell-mount'); setReady({ store, capabilities }); store.start();
    })().catch(error => { if (!cancelled) setError({ signIn: false, message: error instanceof Error ? error.message : String(error) }); });
    return () => { cancelled = true; store?.stop(); };
  }, [attempt]);
  if (!ready) return error ? <DesktopBlocked error={error} onSignIn={() => navigate(`/relay-portal?returnTo=${encodeURIComponent(`${location.pathname}${location.search}${location.hash}`)}`)} onRetry={() => { setError(null); setAttempt(value => value + 1); }} /> : null;
  return <DesktopStoreContext.Provider value={ready.store}><TourProvider><DesktopShell capabilities={ready.capabilities}>{children}</DesktopShell></TourProvider></DesktopStoreContext.Provider>;
}

/** The desktop window has nothing else on screen while it cannot load the account, so
 * the reason comes with its ways out: sign in, retry, or change the address
 * (docs/ui-principles.md §11, the sign-in exception: the reason is visible text). */
function DesktopBlocked({ error, onSignIn, onRetry }: { error: { signIn: boolean; message: string }; onSignIn: () => void; onRetry: () => void }) {
  useI18n();
  const host = desktopHost();
  return <div className="desktop-blocked" role="alert">
    <div className="desktop-blocked-card">
      <h1>{translate(error.signIn ? 'desktopShell.signInTitle' : 'desktopShell.relayUnavailableTitle')}</h1>
      <p>{error.signIn ? translate('desktopShell.signInBody') : <>{translate('desktopShell.relayUnavailableBody')} <ErrorMark message={error.message} onRetry={onRetry} /></>}</p>
      <button type="button" className="desktop-blocked-primary" onClick={error.signIn ? onSignIn : onRetry}>{translate(error.signIn ? 'desktopShell.signIn' : 'common.retry')}</button>
      <div className="desktop-blocked-links">
        {error.signIn ? <button type="button" onClick={onRetry}>{translate('common.retry')}</button> : null}
        {host?.window?.openPreferences ? <button type="button" onClick={() => host.window.openPreferences('general')}>{translate('desktopShell.changeAddress')}</button> : null}
      </div>
    </div>
  </div>;
}
