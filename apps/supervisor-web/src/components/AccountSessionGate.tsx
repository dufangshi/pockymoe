import { useEffect, useState, type ReactNode } from 'react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { currentBrowser, requireUnlock } from '../lib/accountVault';
import { ACCOUNT_CHANGED } from '../lib/accountVaultState';
// One gate shared by all authenticated relay routes, including OAuth callbacks
// and legacy accounts. A dismissed password prompt remains explicitly resumable.
export function AccountSessionGate({ children }: { children: ReactNode }) {
  useI18n(); const [ready, setReady] = useState(false), [error, setError] = useState<string>();
  useEffect(() => {
    let active = true;
    const check = () => { void currentBrowser().then(browser => { if (active) setReady(!!browser); }).catch(e => { if (active) setError(e instanceof Error ? e.message : translate('auth.accountUnlockRequired')); }); };
    void currentBrowser().then(async browser => { if (browser) check(); else { await requireUnlock('connect'); check(); } })
      .catch(e => { if (active) setError(e instanceof Error ? e.message : translate('auth.accountUnlockRequired')); });
    window.addEventListener(ACCOUNT_CHANGED, check);
    return () => { active = false; window.removeEventListener(ACCOUNT_CHANGED, check); };
  }, []);
  if (ready) return children;
  return <main className="product-page px-4 py-24"><p>{translate('auth.accountUnlockHelp')}</p>
    {error && <p role="alert">{error}</p>}
    <button className="relay-button-primary mt-4 min-h-11 px-4" onClick={() => window.dispatchEvent(new Event('remote-codex-owner-unlock-request'))}>{translate('auth.accountUnlock')}</button>
  </main>;
}
