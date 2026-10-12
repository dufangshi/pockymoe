import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@pockymoe/thread-ui';
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { currentAccount, currentBrowser, requireUnlock, type UnlockReason, type UnlockedAccount } from '../lib/accountVault';
import { ACCOUNT_UNLOCK_REQUEST, accountRequest, unlockAccount, registerBrowser, createVault, openVault,
  AccountTrustError, changeAccountPassword, startAccountOver, type Vault } from '../lib/accountVaultState';
import { isOAuthAccount, accountSessionState, setAccountSession, setupAccount, type PreparedRegistration } from '../lib/accountAuth';
import { accountStore } from '../lib/accountStore';
import { clearTransportKeyCache, type TransportStatus } from '../lib/relayTransportCrypto';
import { getTransportStatus, probeDeviceEncryption } from '../lib/relayTransport';
import { OWNER_READ_BLOCKED_EVENT, OWNER_UNLOCKED_EVENT, notifyOwnerUnlocked,
  type OwnerReadBlockedDetail, type OwnerUnlockedDetail } from '../lib/ownerUnlockRecovery';
interface UnlockRequest { reason: UnlockReason; resolve: (a: UnlockedAccount) => void; reject: (e: unknown) => void }
export function RecoveryConfirmation({ code, onConfirm, busy = false }: { code: string; onConfirm: () => void; busy?: boolean }) {
  const [confirmation, setConfirmation] = useState(''); useI18n();
  return <div className="space-y-4">
    <p>{translate('auth.accountSaveRecovery')}</p><code className="block break-all rounded border p-3">{code}</code>
    <label className="block">{translate('auth.accountConfirmRecovery')}<input autoComplete="off" className="relay-input mt-2 w-full" value={confirmation} onChange={e => setConfirmation(e.target.value)} /></label>
    <button className="relay-button-primary min-h-11 px-4" disabled={busy || confirmation.trim().toUpperCase() !== code.slice(-4)} onClick={onConfirm}>{translate('auth.continue')}</button>
  </div>;
}
export function AccountUnlockPrompt() {
  useI18n();
  const [devices, setDevices] = useState<string[]>([]), locked = useRef(new Map<string, boolean>());
  const [request, setRequest] = useState<UnlockRequest | null>(null), queue = useRef<UnlockRequest[]>([]);
  const [needsConnect, setNeedsConnect] = useState(false);
  const [dismissed, setDismissed] = useState(false), [revoked, setRevoked] = useState(false);
  const [recoverySession, setRecoverySession] = useState(false), [newPassword, setNewPassword] = useState('');
  const [secret, setSecret] = useState(''), [recovery, setRecovery] = useState(false), [setup, setSetup] = useState(false);
  const [prepared, setPrepared] = useState<PreparedRegistration>(), [error, setError] = useState<string>(), [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null), previousFocus = useRef<HTMLElement | null>(null);
  const open = !!(devices.length || request || needsConnect) && !dismissed;
  const dismiss = () => {
    setDismissed(true); setSecret(''); setNewPassword(''); setPrepared(undefined); setError(undefined);
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
    window.addEventListener('remote-codex-transport', status); window.addEventListener('remote-codex-owner-unlock-request', status);
    window.addEventListener(OWNER_READ_BLOCKED_EVENT, blocked); window.addEventListener(OWNER_UNLOCKED_EVENT, done);
    window.addEventListener(ACCOUNT_UNLOCK_REQUEST, ask);
    return () => {
      window.removeEventListener('remote-codex-transport', status); window.removeEventListener('remote-codex-owner-unlock-request', status);
      window.removeEventListener(OWNER_READ_BLOCKED_EVENT, blocked); window.removeEventListener(OWNER_UNLOCKED_EVENT, done);
      window.removeEventListener(ACCOUNT_UNLOCK_REQUEST, ask);
      for (const req of queue.current.splice(0)) req.reject(new AccountTrustError('accountUnlockCancelled'));
    };
  }, []);
  useEffect(() => {
    if (!open) return; let active = true;
    setError(undefined); setSetup(false);
    void accountSessionState().then(state => { if (active) { setRecoverySession(!!state?.recovery); if (state?.recovery) setRecovery(true); } });
    accountRequest<Vault>('/relay/account/vault').catch(async e => {
      if (!active) return;
      if (e instanceof AccountTrustError && e.code === 'vault_missing') {
        const cached = await currentAccount(), state = await accountSessionState();
        if (!active) return;
        if (cached || state?.hasVault) setError(translate('auth.accountVaultMissing'));
        else setSetup(true);
      } else setError(e instanceof Error ? e.message : translate('auth.accountUnlockFailed'));
    });
    return () => { active = false; };
  }, [open]);
  async function complete(account: UnlockedAccount) {
    if (!await currentBrowser()) await registerBrowser();
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
    setRequest(null); setNeedsConnect(false); setSecret(''); setNewPassword(''); setPrepared(undefined); setError(undefined);
  }
  async function submit() {
    setBusy(true); setError(undefined);
    try {
      if (setup && !prepared) {
        const id = await accountStore<string>('active'); if (!id) throw new AccountTrustError('accountUnlockRequired');
        setPrepared(await createVault(id, secret)); return;
      }
      if (prepared) {
        await setupAccount(prepared, await isOAuthAccount());
        const saved = prepared.vault; setPrepared(undefined); setSetup(false);
        await complete(await openVault(saved, secret));
      }
      else {
        const account = await unlockAccount(secret, recovery || recoverySession);
        if (recoverySession) {
          await changeAccountPassword(newPassword, await isOAuthAccount());
          const state = await accountSessionState();
          if (state) await setAccountSession(account.userId, { ...state, recovery: false });
        }
        await complete(account);
      }
    } catch (e) { setError(e instanceof Error ? e.message : translate('auth.accountUnlockFailed')); }
    finally { setBusy(false); }
  }
  if (!devices.length && !request && !needsConnect) return null;
  if (dismissed) return createPortal(<aside className="owner-locked-notice" aria-label={translate('auth.accountLocked')}>
    <p role="status">{translate(revoked ? 'auth.browser_revoked' : 'auth.accountLocked')}</p>
    <button className="relay-button-primary account-key-action" onClick={() => window.dispatchEvent(new Event('remote-codex-owner-unlock-request'))}>{translate('auth.accountUnlock')}</button>
  </aside>, document.body);
  // Share Radix's focus-scope stack with the form that requested the unlock.
  // A separate hand-rolled portal remains outside that form's modal focus trap.
  return <Dialog open onOpenChange={next => { if (!next && !busy) dismiss(); }}>
    <DialogContent showCloseButton={false} overlayClassName="!z-[199]"
      className="host-dialog !z-[200] !max-h-[calc(100dvh-32px)] !max-w-[min(28rem,calc(100%-2rem))] space-y-4 overflow-y-auto !rounded-xl border-[var(--theme-border)] !bg-[var(--theme-panel)] !p-5"
      onOpenAutoFocus={event => { event.preventDefault(); previousFocus.current = document.activeElement as HTMLElement | null; input.current?.focus(); }}
      onCloseAutoFocus={event => { event.preventDefault(); if (previousFocus.current?.isConnected) previousFocus.current.focus(); }}
      onPointerDownOutside={event => { if (busy) event.preventDefault(); }}
      onEscapeKeyDown={event => { if (busy) event.preventDefault(); }}>
      <DialogTitle>{translate(setup ? 'auth.accountSecure' : 'auth.accountUnlockTitle')}</DialogTitle>
      <DialogDescription>{translate(revoked ? 'auth.browser_revoked' : setup ? 'auth.accountPasswordHelp' : recoverySession ? 'auth.accountResetRecoveryHelp' : 'auth.accountUnlockHelp')}</DialogDescription>
      {request && <p className="text-sm text-[var(--theme-fg-muted)]">{translate(`auth.accountReason.${request.reason}`)}</p>}
      {prepared ? <RecoveryConfirmation code={prepared.recoveryCode} busy={busy} onConfirm={() => void submit()} /> :
      <form className="space-y-4" onSubmit={e => { e.preventDefault(); void submit(); }}>
        <label className="block">{translate(recovery && !setup ? 'auth.accountRecoveryCode' : 'auth.password')}
          <input ref={input} className="relay-input mt-2 min-h-11 w-full" type={recovery && !setup ? 'text' : 'password'} autoComplete={setup ? 'new-password' : recovery ? 'off' : 'current-password'} minLength={setup ? 12 : undefined} required value={secret} onChange={e => setSecret(e.target.value)} disabled={busy} />
        </label>
        {recoverySession && <label className="block">{translate('auth.newPassword')}<input className="relay-input mt-2 min-h-11 w-full" type="password" autoComplete="new-password" minLength={12} required value={newPassword} disabled={busy} onChange={e => setNewPassword(e.target.value)} /></label>}
        <button className="relay-button-primary account-key-action min-h-11 px-4" disabled={busy || !secret}>{translate(busy ? 'auth.working' : setup ? 'auth.accountSecure' : 'auth.accountUnlock')}</button>
      </form>}
      {error && <p role="alert">{error}</p>}
      <div className="flex gap-4 text-sm">
        {!setup && !recoverySession && <button disabled={busy} onClick={() => { setRecovery(!recovery); setSecret(''); }}>{translate(recovery ? 'auth.accountUsePassword' : 'auth.accountUseRecovery')}</button>}
        {recoverySession && <button disabled={busy} onClick={() => { if (window.confirm(translate('auth.accountResetWarning'))) { setBusy(true); void startAccountOver().then(() => location.assign('/relay-portal')).catch(e => setError(e instanceof Error ? e.message : translate('auth.accountRequestFailed'))).finally(() => setBusy(false)); } }}>{translate('auth.accountStartOver')}</button>}
        <button disabled={busy} onClick={dismiss}>{translate('auth.ownerLater')}</button>
      </div>
    </DialogContent>
  </Dialog>;
}
export async function ensureBrowserAccess() {
  if (!await currentBrowser()) await requireUnlock('connect');
}
