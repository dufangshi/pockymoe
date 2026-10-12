import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { KeyRound, LockKeyhole, ShieldAlert } from 'lucide-react';
import { useCallback, useEffect, useState, type FormEvent, type ReactNode } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { isOAuthAccount, setAccountSession, type AccountSessionState } from '../lib/accountAuth';
import { currentBrowser } from '../lib/accountVault';
import { ACCOUNT_CHANGED, AccountTrustError, accountRevocations, changeAccountPassword, clearBrowser, registerBrowser, startAccountOver, unlockAccount } from '../lib/accountVaultState';
import { fetchRelaySession, relayLogout } from '../lib/api';
import { AccountSetupFlow } from './auth/AccountSetupFlow';
import { AuthCard, AuthField, AuthLink, AuthLoading, AuthNotice, AuthShell, AuthSubmit, SignedInFooter } from './auth/AuthUi';

type Gate =
  | { kind: 'loading' }
  | { kind: 'ready' }
  | { kind: 'setup' }
  | { kind: 'reauth' }
  | { kind: 'unlock'; removed: boolean }
  | { kind: 'reset' }
  | { kind: 'error'; message: string };

const message = (caught: unknown) => caught instanceof Error ? caught.message : translate('auth.accountRequestFailed');

/**
 * One full-page gate in front of every signed-in relay route. It decides from the
 * relay session what this browser still needs (set up the account, sign in again,
 * unlock, or finish a reset) and always offers a way to sign out.
 */
export function AccountSessionGate({ children }: { children: ReactNode }) {
  useI18n();
  const navigate = useNavigate(), location = useLocation();
  const [gate, setGate] = useState<Gate>({ kind: 'loading' });
  const [name, setName] = useState<string | null>(null), [oauth, setOAuth] = useState(false);
  const [signingOut, setSigningOut] = useState(false);

  const evaluate = useCallback(async () => {
    try {
      const session = await fetchRelaySession();
      if (!session.authenticated || !session.user) { goToSignIn(); return; }
      setName(session.user.username || session.user.email || null);
      const account = (session as typeof session & { account?: AccountSessionState & { recentAuth?: boolean } }).account;
      setOAuth(account?.mode === 'oauth' || await isOAuthAccount());
      if (account?.recovery) { setGate({ kind: 'reset' }); return; }
      // Relays without `recentAuth` leave it to the setup request to ask for reauthentication.
      if (!account?.hasVault) { setGate({ kind: account?.recentAuth === false ? 'reauth' : 'setup' }); return; }
      const browser = await currentBrowser();
      if (!browser) { setGate({ kind: 'unlock', removed: false }); return; }
      // A removed browser learns it from the account-signed list and must unlock
      // again; check first so registration never opens a second prompt.
      const revoked = (await accountRevocations(true)).some(r => r.kind === 'browser' && r.subjectId === browser.browserId);
      if (revoked) { await clearBrowser(browser.browserId); setGate({ kind: 'unlock', removed: true }); return; }
      await registerBrowser();
      setGate({ kind: 'ready' });
    } catch (caught) { setGate({ kind: 'error', message: message(caught) }); }
  }, []);

  function goToSignIn() {
    const returnTo = `${location.pathname}${location.search}${location.hash}`;
    navigate(`/relay-portal?returnTo=${encodeURIComponent(returnTo)}`, { replace: true });
  }

  async function signOut() {
    setSigningOut(true);
    try { await relayLogout(); } catch { /* the local session is cleared either way */ }
    finally { setSigningOut(false); goToSignIn(); }
  }

  useEffect(() => { void evaluate(); }, [evaluate]);
  useEffect(() => {
    // Another tab or an in-app prompt may finish the unlock first.
    const changed = () => { void currentBrowser().then(browser => { if (browser) setGate(g => g.kind === 'unlock' ? { kind: 'ready' } : g); }); };
    window.addEventListener(ACCOUNT_CHANGED, changed);
    return () => window.removeEventListener(ACCOUNT_CHANGED, changed);
  }, []);

  const ready = () => { window.dispatchEvent(new Event(ACCOUNT_CHANGED)); setGate({ kind: 'ready' }); };
  const footer = <SignedInFooter busy={signingOut} name={name} onSignOut={() => void signOut()} />;

  switch (gate.kind) {
    case 'ready': return children;
    case 'loading': return <AuthLoading />;
    case 'setup':
      return <AuthShell><AccountSetupFlow footer={footer} oauth={oauth} onDone={ready} onReauthenticate={() => setGate({ kind: 'reauth' })} /></AuthShell>;
    case 'reauth':
      return (
        <AuthShell>
          <AuthCard footer={footer} icon={<ShieldAlert aria-hidden="true" className="h-8 w-8" />} subtitle={translate('auth.ui.reauthBody')} title={translate('auth.ui.reauthTitle')}>
            <button className="relay-button-primary auth-submit h-11 w-full" disabled={signingOut} onClick={() => void signOut()} type="button">
              {translate('auth.ui.reauthAction')}
            </button>
          </AuthCard>
        </AuthShell>
      );
    case 'unlock':
      return <AuthShell><UnlockCard footer={footer} oauth={oauth} onDone={ready} removed={gate.removed} /></AuthShell>;
    case 'reset':
      return <AuthShell><ResetCard footer={footer} oauth={oauth} onDone={ready} onStartedOver={() => { setGate({ kind: 'loading' }); void evaluate(); }} /></AuthShell>;
    case 'error':
      return (
        <AuthShell>
          <AuthCard footer={footer} title={translate('auth.ui.unlockTitle')}>
            <AuthNotice>{gate.message}</AuthNotice>
            <button className="relay-button-secondary h-11 w-full" onClick={() => { setGate({ kind: 'loading' }); void evaluate(); }} type="button">
              {translate('auth.retryConnection')}
            </button>
          </AuthCard>
        </AuthShell>
      );
  }
}

function UnlockCard({ oauth, removed, footer, onDone }: { oauth: boolean; removed: boolean; footer: ReactNode; onDone: () => void }) {
  useI18n();
  const [secret, setSecret] = useState(''), [recovery, setRecovery] = useState(false);
  const [busy, setBusy] = useState(false), [error, setError] = useState<string>();
  const [newPassword, setNewPassword] = useState<'ask' | null>(null);
  if (newPassword) return <NewPasswordCard footer={footer} oauth={oauth} onDone={onDone} subtitle={translate('auth.ui.afterRecoveryBody')} />;
  async function submit(event: FormEvent) {
    event.preventDefault(); setBusy(true); setError(undefined);
    try {
      await unlockAccount(secret, recovery);
      if (recovery) { setNewPassword('ask'); return; }
      await registerBrowser(); onDone();
    } catch (caught) { setError(message(caught)); }
    finally { setBusy(false); }
  }
  return (
    <AuthCard
      footer={footer}
      icon={<LockKeyhole aria-hidden="true" className="h-8 w-8" />}
      subtitle={translate(removed ? 'auth.ui.removedBody' : oauth ? 'auth.ui.unlockBodyOAuth' : 'auth.ui.unlockBody')}
      title={translate(removed ? 'auth.ui.removedTitle' : 'auth.ui.unlockTitle')}
    >
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
        <AuthSubmit busy={busy} disabled={!secret}>{translate('auth.ui.unlock')}</AuthSubmit>
      </form>
    </AuthCard>
  );
}

/** Choose a new password once the vault is open (after a recovery-code unlock). */
function NewPasswordCard({ oauth, footer, subtitle, onDone }: { oauth: boolean; footer: ReactNode; subtitle: string; onDone: () => void }) {
  useI18n();
  const [password, setPassword] = useState(''), [confirm, setConfirm] = useState('');
  const [busy, setBusy] = useState(false), [error, setError] = useState<string>();
  async function submit(event: FormEvent) {
    event.preventDefault(); setError(undefined);
    if (password !== confirm) { setError(translate('auth.ui.passwordMismatch')); return; }
    setBusy(true);
    try {
      await changeAccountPassword(password, oauth);
      await registerBrowser(); onDone();
    } catch (caught) { setError(message(caught)); }
    finally { setBusy(false); }
  }
  return (
    <AuthCard footer={footer} icon={<KeyRound aria-hidden="true" className="h-8 w-8" />} subtitle={subtitle} title={translate('auth.ui.newPasswordTitle')}>
      <form className="space-y-4" onSubmit={submit}>
        <AuthField autoComplete="new-password" autoFocus disabled={busy} hint={translate('auth.ui.passwordRule')} label={translate('auth.newPassword')} minLength={12} onChange={setPassword} type="password" value={password} />
        <AuthField autoComplete="new-password" disabled={busy} label={translate('auth.ui.confirmPassword')} onChange={setConfirm} type="password" value={confirm} />
        {error ? <AuthNotice>{error}</AuthNotice> : null}
        <AuthSubmit busy={busy}>{translate('auth.continue')}</AuthSubmit>
      </form>
    </AuthCard>
  );
}

/** After an administrator reset or a recovery sign-in: recovery code + new password, or start over. */
function ResetCard({ oauth, footer, onDone, onStartedOver }: { oauth: boolean; footer: ReactNode; onDone: () => void; onStartedOver: () => void }) {
  useI18n();
  const [code, setCode] = useState(''), [password, setPassword] = useState(''), [confirm, setConfirm] = useState('');
  const [busy, setBusy] = useState(false), [error, setError] = useState<string>(), [startOver, setStartOver] = useState(false);
  async function submit(event: FormEvent) {
    event.preventDefault(); setError(undefined);
    if (password !== confirm) { setError(translate('auth.ui.passwordMismatch')); return; }
    setBusy(true);
    try {
      const account = await unlockAccount(code, true);
      await changeAccountPassword(password, oauth);
      await setAccountSession(account.userId, { mode: oauth ? 'oauth' : 'account', setupRequired: false, recovery: false, hasVault: true, accountKeyId: account.accountKeyId });
      await registerBrowser(); onDone();
    } catch (caught) { setError(message(caught)); }
    finally { setBusy(false); }
  }
  async function confirmStartOver() {
    setBusy(true); setError(undefined);
    try { await startAccountOver(); onStartedOver(); }
    catch (caught) { setError(message(caught)); setBusy(false); }
  }
  if (startOver) {
    return (
      <AuthCard footer={footer} icon={<ShieldAlert aria-hidden="true" className="h-8 w-8" />} subtitle={translate('auth.ui.startOverBody')} title={translate('auth.ui.startOverTitle')}>
        {error ? <AuthNotice>{error}</AuthNotice> : null}
        <button className="ui-action-danger h-11 w-full rounded-lg font-medium" disabled={busy} onClick={() => void confirmStartOver()} type="button">
          {translate(busy ? 'auth.ui.pleaseWait' : 'auth.ui.startOverAction')}
        </button>
        <button className="relay-button-secondary h-11 w-full" disabled={busy} onClick={() => setStartOver(false)} type="button">{translate('auth.cancel')}</button>
      </AuthCard>
    );
  }
  return (
    <AuthCard footer={footer} icon={<KeyRound aria-hidden="true" className="h-8 w-8" />} subtitle={translate('auth.ui.newPasswordBody')} title={translate('auth.ui.newPasswordTitle')}>
      <form className="space-y-4" onSubmit={submit}>
        <AuthField autoComplete="off" autoFocus disabled={busy} hint={translate('auth.ui.resetCodeHint')} label={translate('auth.ui.resetWithCode')} onChange={setCode} spellCheck={false} value={code} />
        <AuthField autoComplete="new-password" disabled={busy} hint={translate('auth.ui.passwordRule')} label={translate(oauth ? 'auth.ui.unlockPasswordLabel' : 'auth.newPassword')} minLength={12} onChange={setPassword} type="password" value={password} />
        <AuthField autoComplete="new-password" disabled={busy} label={translate('auth.ui.confirmPassword')} onChange={setConfirm} type="password" value={confirm} />
        {error ? <AuthNotice>{error}</AuthNotice> : null}
        <AuthSubmit busy={busy}>{translate('auth.ui.resetAction')}</AuthSubmit>
      </form>
      <div className="text-center text-sm"><AuthLink disabled={busy} onClick={() => setStartOver(true)}>{translate('auth.ui.noRecoveryCode')}</AuthLink></div>
    </AuthCard>
  );
}

export { message as accountErrorMessage };
