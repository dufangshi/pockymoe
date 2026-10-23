import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useMemo, useState, type FormEvent } from 'react';
import { Link, useLocation, useNavigate } from 'react-router-dom';

import type { RelaySessionDto } from '@pockymoe/shared';
import { AccountSetupFlow } from '../components/auth/AccountSetupFlow';
import { AuthCard, AuthDivider, AuthField, AuthLink, AuthLoading, AuthNotice, AuthShell, AuthSubmit, OAuthButtons, RecoveryCodeStep, SignedInFooter, type OAuthProviderName } from '../components/auth/AuthUi';
import { LoginVerification, usePendingLogin } from '../components/RelaySecurity';
import {
  cancelAccountLogin,
  finishAccountLogin,
  isOAuthAccount,
  legacyUpgradeOffer,
  registerAccount,
  startAccountRecovery,
  startLegacyUpgrade,
  startTemporaryResetLogin,
} from '../lib/accountAuth';
import { AccountTrustError, changeAccountPassword, createVault, openVault, registerBrowser } from '../lib/accountVaultState';
import { ApiError, enableRelayMode, fetchRelaySession, relayLogin, relayLogout, startOAuthSignUp } from '../lib/api';

const DEFAULT_RETURN_TO = '/relay-devices';
const RETURN_TO_STORAGE_KEY = 'remote-codex-relay-return-to';

function errorMessage(caught: unknown, fallback = translate('auth.unableToAuthenticateWithTheRelay')) {
  // A rejected password login reaches the browser as a generic 401.
  if (caught instanceof AccountTrustError && caught.code === 'unauthorized') return translate('auth.invalid_credentials');
  if (caught instanceof ApiError && caught.payload.code === 'invalid_invite_code') return translate('auth.invalid_invite_code');
  if (caught instanceof ApiError) return caught.payload.message;
  return caught instanceof Error ? caught.message : fallback;
}

function safeReturnTo(value: unknown) {
  if (typeof value !== 'string' || !value.startsWith('/') || value.startsWith('//')) return null;
  try {
    const base = 'https://pockymoe.invalid';
    const parsed = new URL(value, base);
    if (parsed.origin !== base || parsed.pathname === '/relay-portal' || parsed.pathname.startsWith('/relay-admin')) return null;
    return `${parsed.pathname}${parsed.search}${parsed.hash}`;
  } catch {
    return null;
  }
}

function storedReturnTo() {
  try { return safeReturnTo(window.sessionStorage.getItem(RETURN_TO_STORAGE_KEY)); } catch { return null; }
}

function resolveReturnTo(search: string, state: unknown) {
  const queryValue = safeReturnTo(new URLSearchParams(search).get('returnTo'));
  const stateValue = state && typeof state === 'object' && 'returnTo' in state
    ? safeReturnTo((state as { returnTo?: unknown }).returnTo) : null;
  return queryValue ?? stateValue ?? storedReturnTo() ?? DEFAULT_RETURN_TO;
}

function rememberReturnTo(returnTo: string) {
  try { window.sessionStorage.setItem(RETURN_TO_STORAGE_KEY, returnTo); } catch { /* storage is optional */ }
}

function forgetReturnTo() {
  try { window.sessionStorage.removeItem(RETURN_TO_STORAGE_KEY); } catch { /* nothing to clear */ }
}

function pendingChallenge(result: { authenticator?: boolean; passkey?: boolean }) {
  return { challengeRequired: true as const, authenticator: Boolean(result.authenticator), passkey: Boolean(result.passkey) };
}

/** What happens right after the relay accepted a sign-in or a registration. */
type Finished =
  | { kind: 'setup'; oauth: boolean; password?: string | undefined; name: string | null }
  | { kind: 'recovery-code'; code: string; pending: boolean };

export function RelayPortalPage() {
  useI18n();
  const navigate = useNavigate();
  const location = useLocation();
  const returnTo = useMemo(() => resolveReturnTo(location.search, location.state), [location.search, location.state]);
  const [session, setSession] = useState<RelaySessionDto | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [finished, setFinished] = useState<Finished | null>(null);
  // An administrator session from before the console had its own cookie.
  const [admin, setAdmin] = useState(false);

  function continueToRequestedPage() {
    forgetReturnTo();
    navigate(returnTo, { replace: true });
  }

  async function load() {
    setLoading(true); setError(null);
    try {
      enableRelayMode();
      const next = await fetchRelaySession();
      // The administrator has no devices: offer the console instead of
      // silently signing it out (which also ended its console session).
      if (next.authenticated && next.user?.role === 'admin') { setAdmin(true); setSession(null); return; }
      setAdmin(false);
      setSession(next);
      // The account gate in front of the app finishes setup, unlock and resets.
      if (next.authenticated) continueToRequestedPage();
    } catch (caught) {
      setSession(null);
      setError(errorMessage(caught, translate('auth.unableToLoadTheRelayPortal')));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => { rememberReturnTo(returnTo); void load(); }, []);

  /** Called once the relay accepted the credentials (and any second factor). */
  async function handleAuthenticated(password?: string) {
    enableRelayMode();
    const next = await fetchRelaySession();
    if (next.user?.role === 'admin') {
      navigate('/relay-admin', { replace: true });
      return;
    }
    if (!next.user) throw new Error(translate('auth.accountUnlockRequired'));
    const flow = await finishAccountLogin(next.user.id, next.user);
    if (flow.setup) {
      setFinished({ kind: 'setup', oauth: await isOAuthAccount(), password, name: next.user.username || next.user.email || null });
      return;
    }
    continueToRequestedPage();
  }

  function signOutOfSetup() {
    void relayLogout().catch(() => undefined).finally(() => { setFinished(null); void load(); });
  }

  if (loading) return <AuthLoading />;
  if (finished?.kind === 'setup') {
    return (
      <AuthShell>
        <AccountSetupFlow
          footer={<SignedInFooter name={finished.name} onSignOut={signOutOfSetup} />}
          initialPassword={finished.password}
          oauth={finished.oauth}
          onDone={continueToRequestedPage}
          onReauthenticate={signOutOfSetup}
        />
      </AuthShell>
    );
  }
  if (finished?.kind === 'recovery-code') {
    return (
      <AuthShell>
        <RecoveryCodeStep code={finished.code} onDone={() => {
          if (!finished.pending) { continueToRequestedPage(); return; }
          setFinished(null); setNotice(translate('auth.ui.pendingApproval'));
        }} />
      </AuthShell>
    );
  }
  if (admin) {
    return (
      <AuthShell>
        <AuthCard
          footer={<AuthLink onClick={() => void relayLogout().catch(() => undefined).finally(() => { setAdmin(false); void load(); })}>{translate('auth.ui.signOut')}</AuthLink>}
          subtitle={translate('auth.ui.adminSessionBody')}
          title={translate('auth.ui.adminSessionTitle')}
        >
          <Link className="relay-button-primary inline-flex h-11 w-full items-center justify-center" to="/relay-admin">{translate('auth.ui.openAdminConsole')}</Link>
        </AuthCard>
      </AuthShell>
    );
  }
  if (session?.authenticated) return <AuthLoading />;

  const params = new URLSearchParams(location.search);
  return (
    <AuthShell>
      <SignInPanel
        initialError={error ?? params.get('oauthError')}
        initialNotice={notice ?? (params.has('oauthPending') ? translate('auth.oAuthRegistrationReceivedAnAdminMustApprove') : null)}
        onAuthenticated={handleAuthenticated}
        onRegistered={(code, pending) => setFinished({ kind: 'recovery-code', code, pending })}
        onRetry={error ? load : undefined}
        registrationEnabled={session?.registrationEnabled ?? false}
        settings={session?.registrationSettings}
      />
    </AuthShell>
  );
}

type View = 'signin' | 'register' | 'reset';

function SignInPanel({ registrationEnabled, settings, initialError, initialNotice, onAuthenticated, onRegistered, onRetry }: {
  registrationEnabled: boolean;
  settings: RelaySessionDto['registrationSettings'];
  initialError: string | null;
  initialNotice: string | null;
  onAuthenticated: (password?: string) => Promise<void>;
  onRegistered: (recoveryCode: string, pendingApproval: boolean) => void;
  onRetry: (() => Promise<void>) | undefined;
}) {
  useI18n();
  const navigate = useNavigate();
  const [view, setView] = useState<View>('signin');
  const [challenge, setChallenge] = usePendingLogin();
  const [identifier, setIdentifier] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(initialError);
  const [notice, setNotice] = useState<string | null>(initialNotice);
  const [busy, setBusy] = useState(false);
  // Shown after a failed sign-in: older accounts finish the upgrade with their old password.
  const [legacy, setLegacy] = useState<null | { otherVault: boolean }>(null);
  const [legacyConfirmed, setLegacyConfirmed] = useState(false);
  // The password a legacy upgrade signed in with, reused to create the vault.
  const [upgradePassword, setUpgradePassword] = useState<string>();

  useEffect(() => { setError(initialError); }, [initialError]);
  useEffect(() => { setNotice(initialNotice); }, [initialNotice]);
  function go(next: View) { setView(next); setError(null); setNotice(null); setLegacy(null); setPassword(''); }

  async function signIn(event: FormEvent) {
    event.preventDefault(); setBusy(true); setError(null); setNotice(null); setLegacy(null);
    try {
      const result = await relayLogin({ identifier, password });
      if (result.challengeRequired) { setChallenge(pendingChallenge(result)); return; }
      if (result.session?.user?.role === 'admin') { navigate('/relay-admin'); return; }
      setPassword('');
      await onAuthenticated();
    } catch (caught) {
      setError(errorMessage(caught));
      // A not-yet-secured account fails the derived-key sign-in like a wrong password.
      if (caught instanceof AccountTrustError && caught.code === 'unauthorized') {
        setLegacy(await legacyUpgradeOffer(identifier)); setLegacyConfirmed(false);
      }
    } finally { setBusy(false); }
  }

  async function upgrade() {
    setBusy(true); setError(null);
    try {
      const result = await startLegacyUpgrade(identifier, password, legacy?.otherVault ?? false);
      // The relay administrator signed in: its place is the admin console.
      if (result.session?.user?.role === 'admin') { setPassword(''); navigate('/relay-admin'); return; }
      const typed = password;
      setUpgradePassword(typed); setPassword(''); setLegacy(null);
      if (result.challengeRequired) { setChallenge(pendingChallenge(result)); return; }
      await onAuthenticated(typed);
    } catch (caught) { setError(errorMessage(caught)); }
    finally { setBusy(false); }
  }

  if (challenge) {
    return (
      <LoginVerification
        challenge={challenge}
        onBack={() => { cancelAccountLogin(); setChallenge(null); setUpgradePassword(undefined); }}
        onSuccess={() => onAuthenticated(upgradePassword)}
      />
    );
  }
  if (view === 'register') return <RegisterCard onBack={() => go('signin')} onRegistered={onRegistered} settings={settings} />;
  if (view === 'reset') return <ResetPasswordCard onAuthenticated={onAuthenticated} onBack={() => go('signin')} onChallenge={setChallenge} />;

  const oauth = settings?.googleAuthEnabled || settings?.githubAuthEnabled;
  return (
    <AuthCard
      footer={
        <span className="flex flex-col items-center gap-2">
          {registrationEnabled
            ? <span>{translate('auth.ui.newHere')} <AuthLink onClick={() => go('register')}>{translate('auth.createAccount')}</AuthLink></span>
            : null}
          <span className="auth-admin-link text-xs">
            {translate('auth.ui.adminQuestion')}{' '}
            <Link className="font-medium text-[var(--theme-accent-strong)] hover:underline" to="/relay-admin">{translate('auth.ui.adminConsole')}</Link>
          </span>
        </span>
      }
      title={translate('auth.ui.signInTitle')}
    >
      {notice ? <AuthNotice tone="info">{notice}</AuthNotice> : null}
      {oauth ? <><OAuthButtons github={settings?.githubAuthEnabled} google={settings?.googleAuthEnabled} /><AuthDivider /></> : null}
      <form className="space-y-4" onSubmit={signIn}>
        <AuthField autoComplete="username" autoFocus disabled={busy} label={translate('auth.emailOrUsername')} name="identifier" onChange={setIdentifier} spellCheck={false} value={identifier} />
        <AuthField
          aside={<AuthLink disabled={busy} onClick={() => go('reset')}>{translate('auth.ui.forgotPassword')}</AuthLink>}
          autoComplete="current-password" disabled={busy} label={translate('auth.password')} name="password" onChange={setPassword} type="password" value={password}
        />
        {legacy ? (
          <div className="auth-legacy space-y-3 rounded-xl border border-[var(--theme-border)] p-3.5 text-sm">
            <p className="font-medium text-[var(--theme-fg)]">{translate('auth.ui.legacyTitle')}</p>
            <p className="text-[var(--theme-fg-muted)]">{translate(legacy.otherVault ? 'auth.ui.legacyOtherAccount' : 'auth.ui.legacyBody')}</p>
            {legacy.otherVault ? (
              <label className="flex items-start gap-2 text-[var(--theme-fg)]">
                <input checked={legacyConfirmed} className="mt-1" disabled={busy} onChange={e => setLegacyConfirmed(e.target.checked)} type="checkbox" />
                {translate('auth.accountUpgradeExistingVaultConfirm')}
              </label>
            ) : null}
            <button className="relay-button-secondary h-10 w-full" disabled={busy || !password || (legacy.otherVault && !legacyConfirmed)} onClick={() => void upgrade()} type="button">
              {translate('auth.ui.legacyAction')}
            </button>
          </div>
        ) : null}
        <AuthSubmit busy={busy} disabled={!identifier || !password} error={error}
          onRetry={onRetry && error === initialError ? () => void onRetry() : undefined}>{translate('auth.signIn')}</AuthSubmit>
      </form>
    </AuthCard>
  );
}

function RegisterCard({ settings, onBack, onRegistered }: {
  settings: RelaySessionDto['registrationSettings'];
  onBack: () => void;
  onRegistered: (recoveryCode: string, pendingApproval: boolean) => void;
}) {
  useI18n();
  const [email, setEmail] = useState(''), [username, setUsername] = useState('');
  const [password, setPassword] = useState(''), [confirm, setConfirm] = useState(''), [code, setCode] = useState('');
  const [busy, setBusy] = useState(false), [error, setError] = useState<string | null>(null);
  const [oauthBusy, setOAuthBusy] = useState<OAuthProviderName | null>(null);
  const [codeError, setCodeError] = useState<string | null>(null);
  const codeRequired = settings?.registrationPasswordConfigured ?? Boolean(settings?.registrationPassword);

  /** Google or GitHub sign-up with the invite code: the relay keeps the code
      with the pending sign-in and checks it when the provider returns. */
  async function startOAuth(provider: OAuthProviderName) {
    setCodeError(null);
    if (!code.trim()) { setCodeError(translate('auth.ui.inviteCodeFirst')); return; }
    setOAuthBusy(provider);
    try {
      window.location.assign(await startOAuthSignUp(provider, code.trim()));
    } catch (caught) {
      setCodeError(errorMessage(caught));
      setOAuthBusy(null);
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault(); setError(null);
    if (codeRequired && !code.trim()) { setCodeError(translate('auth.ui.inviteCodeFirst')); return; }
    if (username.trim().length < 3) { setError(translate('auth.usernameMustBeAtLeast3Characters')); return; }
    if (password !== confirm) { setError(translate('auth.ui.passwordMismatch')); return; }
    setBusy(true);
    try {
      // The vault is stored with the account, so the recovery code is valid once shown.
      const prepared = await createVault(crypto.randomUUID(), password);
      const trimmed = code.trim();
      const result = await registerAccount({ email, username, ...(trimmed ? { registrationPassword: trimmed } : {}) }, prepared);
      if (!result.pendingApproval) { await openVault(prepared.vault, password); await registerBrowser(undefined, 'registration'); }
      onRegistered(prepared.recoveryCode, Boolean(result.pendingApproval));
    } catch (caught) { setError(errorMessage(caught)); }
    finally { setBusy(false); }
  }

  const oauth = settings?.googleAuthEnabled || settings?.githubAuthEnabled;
  return (
    <AuthCard
      footer={<span>{translate('auth.ui.haveAccount')} <AuthLink onClick={onBack}>{translate('auth.signIn')}</AuthLink></span>}
      title={translate('auth.ui.registerTitle')}
    >
      {codeRequired ? (
        // Every way to create an account needs the code, so it comes first.
        <div className="auth-invite space-y-1.5">
          <AuthField autoComplete="one-time-code" autoFocus disabled={busy || Boolean(oauthBusy)} hint={translate('auth.ui.inviteCodeHint')} label={translate('auth.registrationCode')} name="registrationCode" onChange={(value) => { setCode(value); setCodeError(null); }} value={code} />
          {codeError ? <p className="auth-field-error text-xs text-[var(--status-warning-fg)]" role="alert">{codeError}</p> : null}
        </div>
      ) : null}
      {oauth ? <><OAuthButtons busy={oauthBusy} github={settings?.githubAuthEnabled} google={settings?.googleAuthEnabled} onStart={codeRequired ? (provider) => void startOAuth(provider) : undefined} /><AuthDivider /></> : null}
      <form className="space-y-4" onSubmit={submit}>
        <AuthField autoComplete="email" autoFocus={!codeRequired} disabled={busy} inputMode="email" label={translate('auth.email')} name="email" onChange={setEmail} type="email" value={email} />
        <AuthField autoComplete="username" disabled={busy} label={translate('auth.username')} minLength={3} name="username" onChange={setUsername} spellCheck={false} value={username} />
        <AuthField autoComplete="new-password" disabled={busy} hint={translate('auth.ui.passwordRule')} label={translate('auth.password')} minLength={12} name="password" onChange={setPassword} type="password" value={password} />
        <AuthField autoComplete="new-password" disabled={busy} label={translate('auth.ui.confirmPassword')} name="confirm" onChange={setConfirm} type="password" value={confirm} />
        {!codeRequired ? <AuthField autoComplete="one-time-code" disabled={busy} label={translate('auth.ui.registrationCodeOptional')} name="registrationCode" onChange={setCode} required={false} value={code} /> : null}
        <AuthSubmit busy={busy} error={error}>{translate('auth.createAccount')}</AuthSubmit>
      </form>
    </AuthCard>
  );
}

function ResetPasswordCard({ onBack, onAuthenticated, onChallenge }: {
  onBack: () => void;
  onAuthenticated: (password?: string) => Promise<void>;
  onChallenge: (challenge: ReturnType<typeof pendingChallenge>) => void;
}) {
  useI18n();
  const [method, setMethod] = useState<'code' | 'temporary'>('code');
  const [identifier, setIdentifier] = useState(''), [secret, setSecret] = useState('');
  const [password, setPassword] = useState(''), [confirm, setConfirm] = useState('');
  const [busy, setBusy] = useState(false), [error, setError] = useState<string | null>(null);

  async function submit(event: FormEvent) {
    event.preventDefault(); setError(null);
    if (method === 'code' && password !== confirm) { setError(translate('auth.ui.passwordMismatch')); return; }
    setBusy(true);
    try {
      if (method === 'temporary') {
        // The account gate then asks for the recovery code and a new password.
        const result = await startTemporaryResetLogin(identifier, secret); setSecret('');
        if (result.challengeRequired) { onChallenge(pendingChallenge(result)); return; }
        await onAuthenticated(); return;
      }
      const result = await startAccountRecovery(identifier, secret);
      if (result.challengeRequired) { onChallenge(pendingChallenge(result)); return; }
      const session = await fetchRelaySession();
      if (!session.user) throw new Error(translate('auth.accountUnlockRequired'));
      await finishAccountLogin(session.user.id, session.user);
      await changeAccountPassword(password, await isOAuthAccount());
      await registerBrowser(undefined, 'sign-in');
      await onAuthenticated();
    } catch (caught) { setError(errorMessage(caught)); }
    finally { setBusy(false); }
  }

  return (
    <AuthCard footer={<AuthLink onClick={onBack}>{translate('auth.ui.backToSignIn')}</AuthLink>} title={translate('auth.ui.resetTitle')}>
      <div aria-label={translate('auth.ui.resetTitle')} className="auth-segment grid grid-cols-2 rounded-xl bg-[var(--theme-muted)] p-1 text-sm" role="tablist">
        {(['code', 'temporary'] as const).map(value => (
          <button aria-selected={method === value} className={`min-h-10 rounded-lg px-3 font-medium transition ${method === value ? 'bg-[var(--theme-panel)] text-[var(--theme-fg)] shadow-sm' : 'text-[var(--theme-fg-muted)] hover:text-[var(--theme-fg)]'}`}
            disabled={busy} key={value} onClick={() => { setMethod(value); setSecret(''); setError(null); }} role="tab" type="button">
            {translate(value === 'code' ? 'auth.ui.resetWithCode' : 'auth.ui.resetWithTemporary')}
          </button>
        ))}
      </div>
      <form className="space-y-4" onSubmit={submit}>
        <AuthField autoComplete="username" autoFocus disabled={busy} label={translate('auth.emailOrUsername')} onChange={setIdentifier} spellCheck={false} value={identifier} />
        <AuthField
          autoComplete="off" disabled={busy} key={method}
          hint={translate(method === 'code' ? 'auth.ui.resetCodeHint' : 'auth.ui.resetTemporaryHint')}
          label={translate(method === 'code' ? 'auth.ui.resetWithCode' : 'auth.accountTemporaryPassword')}
          onChange={setSecret} spellCheck={false} type={method === 'code' ? 'text' : 'password'} value={secret}
        />
        {method === 'code' ? <>
          <AuthField autoComplete="new-password" disabled={busy} hint={translate('auth.ui.passwordRule')} label={translate('auth.newPassword')} minLength={12} onChange={setPassword} type="password" value={password} />
          <AuthField autoComplete="new-password" disabled={busy} label={translate('auth.ui.confirmPassword')} onChange={setConfirm} type="password" value={confirm} />
        </> : null}
        <AuthSubmit busy={busy} error={error}>{translate(method === 'code' ? 'auth.ui.resetAction' : 'auth.signIn')}</AuthSubmit>
      </form>
    </AuthCard>
  );
}
