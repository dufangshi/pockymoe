import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@pockymoe/thread-ui';
import { LockKeyhole } from 'lucide-react';
import { useEffect, useRef, useState, type FormEvent } from 'react';
import { createPortal } from 'react-dom';
import { currentBrowser, requireUnlock, type UnlockReason, type UnlockedAccount } from '../lib/accountVault';
import { ACCOUNT_UNLOCK_REQUEST, BROWSER_REMOVED, AccountTrustError, registerBrowser, unlockAccount } from '../lib/accountVaultState';
import { accountSessionState, isOAuthAccount } from '../lib/accountAuth';
import { relayLogout } from '../lib/api';
import { clearTransportKeyCache, type TransportStatus } from '../lib/relayTransportCrypto';
import { getTransportStatus, probeDeviceEncryption } from '../lib/relayTransport';
import { OWNER_READ_BLOCKED_EVENT, OWNER_UNLOCKED_EVENT, notifyOwnerUnlocked,
  type OwnerReadBlockedDetail, type OwnerUnlockedDetail } from '../lib/ownerUnlockRecovery';
import { AuthField, AuthLink, AuthNotice, RecoveryCodeBody } from './auth/AuthUi';

interface UnlockRequest { reason: UnlockReason; resolve: (a: UnlockedAccount) => void; reject: (e: unknown) => void }

/** Kept for the account security panel: a regenerated recovery code. */
export function RecoveryConfirmation({ code, onConfirm, busy = false }: { code: string; onConfirm: () => void; busy?: boolean }) {
  return <RecoveryCodeBody busy={busy} code={code} onDone={onConfirm} />;
}

/**
 * The in-app unlock prompt: sensitive actions (requireUnlock) and devices that
 * report this browser as locked. Account setup, sign-in again and resets are
 * handled by the full-page account gate before the app opens.
 */
export function AccountUnlockPrompt() {
  useI18n();
  const [devices, setDevices] = useState<string[]>([]), locked = useRef(new Map<string, boolean>());
  const [request, setRequest] = useState<UnlockRequest | null>(null), queue = useRef<UnlockRequest[]>([]);
  const [needsConnect, setNeedsConnect] = useState(false);
  const [dismissed, setDismissed] = useState(false), [lockedRevoked, setRevoked] = useState(false);
  const [browserRemoved, setBrowserRemoved] = useState(false), revoked = lockedRevoked || browserRemoved;
  const [secret, setSecret] = useState(''), [recovery, setRecovery] = useState(false), [oauth, setOAuth] = useState(false);
  const [setupPending, setSetupPending] = useState(false);
  const [error, setError] = useState<string>(), [busy, setBusy] = useState(false);
  const previousFocus = useRef<HTMLElement | null>(null);
  const open = !!(devices.length || request || needsConnect) && !dismissed;
  const reset = () => { setSecret(''); setRecovery(false); setError(undefined); };
  const dismiss = () => {
    setDismissed(true); reset();
    for (const req of queue.current.splice(0)) req.reject(new AccountTrustError('accountUnlockCancelled'));
    setRequest(null);
  };
  useEffect(() => {
    const status = (event: Event) => {
      if (event.type === 'remote-codex-owner-unlock-request') setDismissed(false);
      const s = (event as CustomEvent<TransportStatus>).detail; if (s?.state !== 'unlock-required') return;
      const wasRevoked = locked.current.get(s.deviceId) === true, removed = s.unlockReason === 'browser_revoked';
      locked.current.set(s.deviceId, wasRevoked || removed);
      if (removed && !wasRevoked) setDismissed(false);
      setDevices([...locked.current.keys()]); setRevoked([...locked.current.values()].some(Boolean));
    };
    const blocked = (event: Event) => {
      const d = (event as CustomEvent<OwnerReadBlockedDetail>).detail;
      status(new CustomEvent('remote-codex-transport', { detail: { deviceId: d.deviceId, state: 'unlock-required', ...(d.revoked ? { unlockReason: 'browser_revoked' } : {}) } }));
    };
    const done = (event: Event) => {
      for (const id of (event as CustomEvent<OwnerUnlockedDetail>).detail?.deviceIds ?? []) locked.current.delete(id);
      setDevices([...locked.current.keys()]); setRevoked([...locked.current.values()].some(Boolean));
    };
    const ask = (event: Event) => {
      const req = (event as CustomEvent<UnlockRequest>).detail; queue.current.push(req); if (req.reason === 'connect') setNeedsConnect(true); setRequest(req); setDismissed(false);
    };
    const removedBrowser = () => { setBrowserRemoved(true); setDismissed(false); };
    window.addEventListener('remote-codex-transport', status); window.addEventListener('remote-codex-owner-unlock-request', status);
    window.addEventListener(OWNER_READ_BLOCKED_EVENT, blocked); window.addEventListener(OWNER_UNLOCKED_EVENT, done);
    window.addEventListener(ACCOUNT_UNLOCK_REQUEST, ask); window.addEventListener(BROWSER_REMOVED, removedBrowser);
    return () => {
      window.removeEventListener(BROWSER_REMOVED, removedBrowser);
      window.removeEventListener('remote-codex-transport', status); window.removeEventListener('remote-codex-owner-unlock-request', status);
      window.removeEventListener(OWNER_READ_BLOCKED_EVENT, blocked); window.removeEventListener(OWNER_UNLOCKED_EVENT, done);
      window.removeEventListener(ACCOUNT_UNLOCK_REQUEST, ask);
      for (const req of queue.current.splice(0)) req.reject(new AccountTrustError('accountUnlockCancelled'));
    };
  }, []);
  useEffect(() => {
    if (!open) return; let active = true;
    reset();
    void Promise.all([accountSessionState(), isOAuthAccount()]).then(([state, oauthOnly]) => {
      if (!active) return;
      setOAuth(oauthOnly);
      // Setup and resets belong to the account gate; never collect a password here for them.
      setSetupPending(!!state && (!state.hasVault || state.recovery));
    });
    return () => { active = false; };
  }, [open]);

  async function complete(account: UnlockedAccount) {
    await registerBrowser(); setBrowserRemoved(false);
    // Only reissue the original reads after a device confirms the new identity.
    const waitingDevices = [...locked.current.keys()];
    for (const id of waitingDevices) {
      clearTransportKeyCache(id); navigator.serviceWorker?.controller?.postMessage({ type: 'remote-codex-reset-transport', deviceId: id });
      await probeDeviceEncryption(id);
      if (getTransportStatus(id)?.state !== 'encrypted') throw new AccountTrustError('accountConnectionFailed');
      notifyOwnerUnlocked([id]);
    }
    if (!waitingDevices.length) notifyOwnerUnlocked([]);
    for (const req of queue.current.splice(0)) req.resolve(account);
    setRequest(null); setNeedsConnect(false); reset();
  }
  async function submit(event: FormEvent) {
    event.preventDefault(); setBusy(true); setError(undefined);
    try { await complete(await unlockAccount(secret, recovery)); }
    catch (e) { setError(e instanceof Error ? e.message : translate('auth.accountUnlockFailed')); }
    finally { setBusy(false); }
  }
  async function signOut() {
    setBusy(true);
    try { await relayLogout(); } catch { /* the local session is cleared either way */ }
    window.location.assign('/relay-portal');
  }

  if (!devices.length && !request && !needsConnect) return null;
  if (dismissed) return createPortal(<aside className="owner-locked-notice" aria-label={translate('auth.accountLocked')}>
    <p role="status">{translate(revoked ? 'auth.ui.removedTitle' : 'auth.accountLocked')}</p>
    <button className="relay-button-primary account-key-action" onClick={() => window.dispatchEvent(new Event('remote-codex-owner-unlock-request'))}>{translate('auth.ui.unlock')}</button>
  </aside>, document.body);

  const title = revoked ? 'auth.ui.removedTitle' : request && request.reason !== 'connect' ? 'auth.ui.confirmTitle' : 'auth.ui.unlockTitle';
  const description = setupPending ? 'auth.ui.setupElsewhere' : revoked ? 'auth.ui.removedBody' : oauth ? 'auth.ui.unlockBodyOAuth' : 'auth.ui.unlockBody';
  // Share Radix's focus-scope stack with the form that requested the unlock.
  return <Dialog open onOpenChange={next => { if (!next && !busy) dismiss(); }}>
    <DialogContent showCloseButton={false} overlayClassName="!z-[199]"
      className="host-dialog auth-dialog !z-[200] !max-h-[calc(100dvh-32px)] !max-w-[min(25rem,calc(100%-2rem))] space-y-5 overflow-y-auto !rounded-2xl border-[var(--theme-border)] !bg-[var(--theme-panel)] !p-6"
      onOpenAutoFocus={event => { previousFocus.current = document.activeElement as HTMLElement | null; if (setupPending) event.preventDefault(); }}
      onCloseAutoFocus={event => { event.preventDefault(); if (previousFocus.current?.isConnected) previousFocus.current.focus(); }}
      onPointerDownOutside={event => { if (busy) event.preventDefault(); }}
      onEscapeKeyDown={event => { if (busy) event.preventDefault(); }}>
      <div className="space-y-2">
        <LockKeyhole aria-hidden="true" className="h-7 w-7 text-[var(--theme-accent-strong)]" />
        <DialogTitle className="text-lg font-semibold">{translate(title)}</DialogTitle>
        <DialogDescription className="text-sm leading-6 text-[var(--theme-fg-muted)]">{translate(description)}</DialogDescription>
      </div>
      {setupPending ? (
        <button className="relay-button-primary h-11 w-full" onClick={() => window.location.reload()} type="button">{translate('auth.ui.setupContinue')}</button>
      ) : (
        <form className="space-y-4" onSubmit={submit}>
          <AuthField
            aside={<AuthLink disabled={busy} onClick={() => { setRecovery(!recovery); setSecret(''); setError(undefined); }}>{translate(recovery ? 'auth.ui.usePassword' : 'auth.ui.useRecoveryCode')}</AuthLink>}
            autoComplete={recovery ? 'off' : 'current-password'}
            autoFocus
            disabled={busy}
            key={recovery ? 'recovery' : 'password'}
            label={translate(recovery ? 'auth.ui.resetWithCode' : oauth ? 'auth.ui.unlockPasswordLabel' : 'auth.password')}
            onChange={setSecret}
            spellCheck={false}
            type={recovery ? 'text' : 'password'}
            value={secret}
          />
          {error ? <AuthNotice>{error}</AuthNotice> : null}
          <div className="flex gap-2">
            <button className="relay-button-secondary h-11 flex-1" disabled={busy} onClick={dismiss} type="button">{translate('auth.ui.later')}</button>
            <button aria-busy={busy || undefined} className="relay-button-primary account-key-action h-11 flex-1" disabled={busy || !secret} type="submit">
              {translate(busy ? 'auth.ui.pleaseWait' : 'auth.ui.unlock')}
            </button>
          </div>
        </form>
      )}
      <div className="border-t border-[var(--theme-border)] pt-4 text-center text-sm text-[var(--theme-fg-muted)]">
        <AuthLink disabled={busy} onClick={() => void signOut()}>{translate('auth.ui.signOut')}</AuthLink>
      </div>
    </DialogContent>
  </Dialog>;
}

export async function ensureBrowserAccess() {
  if (!await currentBrowser()) await requireUnlock('connect');
}
