import { finishAccountLogin, startLegacyUpgrade, legacyUpgradeEnabled, legacyUpgradeMayApply, hasOpenedAccountVault, registerAccount, startAccountRecovery, startTemporaryResetLogin, cancelAccountLogin, isOAuthAccount, type PreparedRegistration } from '../lib/accountAuth';
import { requireUnlock } from '../lib/accountVault';
import { ensureBrowserAccess, RecoveryConfirmation } from '../components/AccountUnlockPrompt';
import { createVault, openVault, registerBrowser, changeAccountPassword, AccountTrustError } from '../lib/accountVaultState';
import { relayUrl } from '../lib/relayOrigin';
import { LanguageSwitcher } from '@pockymoe/thread-ui/i18n';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { LoginVerification, usePendingLogin } from '../components/RelaySecurity';
import { ArrowLeft, BookOpen, Eye, EyeOff } from 'lucide-react';
import {
  FormEvent,
  useEffect,
  useId,
  useMemo,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';
import { Link, useLocation, useNavigate } from 'react-router-dom';

import type { RelaySessionDto } from '@pockymoe/shared';
import {
  ApiError,
  enableRelayMode,
  fetchRelaySession,
  relayLogin,
  relayLogout,
} from '../lib/api';

type AuthMode = 'login' | 'register' | 'recover' | 'reset' | 'upgrade';

const DEFAULT_RETURN_TO = '/relay-devices';
const RETURN_TO_STORAGE_KEY = 'remote-codex-relay-return-to';

function errorMessage(caught: unknown, fallback: string) {
  return caught instanceof ApiError
    ? caught.payload.message
    : caught instanceof Error
      ? caught.message
      : fallback;
}

function safeReturnTo(value: unknown) {
  if (
    typeof value !== 'string' ||
    !value.startsWith('/') ||
    value.startsWith('//')
  ) {
    return null;
  }

  try {
    const base = 'https://pockymoe.invalid';
    const parsed = new URL(value, base);
    const path = `${parsed.pathname}${parsed.search}${parsed.hash}`;
    if (
      parsed.origin !== base ||
      parsed.pathname === '/relay-portal' ||
      parsed.pathname.startsWith('/relay-admin')
    ) {
      return null;
    }
    return path;
  } catch {
    return null;
  }
}

function storedReturnTo() {
  if (typeof window === 'undefined') {
    return null;
  }
  try {
    return safeReturnTo(window.sessionStorage.getItem(RETURN_TO_STORAGE_KEY));
  } catch {
    return null;
  }
}

function resolveReturnTo(search: string, state: unknown) {
  const queryValue = safeReturnTo(new URLSearchParams(search).get('returnTo'));
  const stateValue =
    state && typeof state === 'object' && 'returnTo' in state
      ? safeReturnTo((state as { returnTo?: unknown }).returnTo)
      : null;
  return queryValue ?? stateValue ?? storedReturnTo() ?? DEFAULT_RETURN_TO;
}

function rememberReturnTo(returnTo: string) {
  if (typeof window === 'undefined') {
    return;
  }
  try {
    window.sessionStorage.setItem(RETURN_TO_STORAGE_KEY, returnTo);
  } catch {
    // Authentication still works when storage is unavailable.
  }
}

function forgetReturnTo() {
  if (typeof window === 'undefined') {
    return;
  }
  try {
    window.sessionStorage.removeItem(RETURN_TO_STORAGE_KEY);
  } catch {
    // There may be nothing to clear in restricted browser contexts.
  }
}

export function RelayPortalPage() {
  const { locale: i18nLocale } = useI18n();
  const navigate = useNavigate();
  const location = useLocation();
  const returnTo = useMemo(
    () => resolveReturnTo(location.search, location.state),
    [location.search, location.state],
  );
  const [session, setSession] = useState<RelaySessionDto | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  function continueToRequestedPage() {
    forgetReturnTo();
    navigate(returnTo, { replace: true });
  }

  async function load() {
    setLoading(true);
    setError(null);
    try {
      enableRelayMode();
      const nextSession = await fetchRelaySession();
      if (nextSession.authenticated && nextSession.user?.role === 'admin') {
        const signedOutSession = await relayLogout();
        setSession(signedOutSession);
        return;
      }
      setSession(nextSession);
      if (nextSession.authenticated) {
        continueToRequestedPage();
      }
    } catch (caught) {
      setSession(null);
      setError(errorMessage(caught, translate("auth.unableToLoadTheRelayPortal")));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    rememberReturnTo(returnTo);
    void load();
  }, []);

  async function handleAuthenticated() {
    enableRelayMode();
    const nextSession = await fetchRelaySession();
    if (nextSession.user?.role === 'admin') {
      const signedOutSession = await relayLogout();
      setSession(signedOutSession);
      throw new Error(translate("auth.thisPortalAcceptsRelayUserAccountsOnly"));
    }
    if (!nextSession.user) throw new Error(translate('auth.accountUnlockRequired'));
    const flow = await finishAccountLogin(nextSession.user.id, nextSession.user);
    continueToRequestedPage();
  }

  if (loading) {
    return (
      <RelayFrame>
        <div
          aria-live="polite"
          className="w-full max-w-md py-12 text-sm text-[var(--theme-fg-muted)]"
          role="status"
        >
          {translate("auth.checkingRelaySession")}</div>
      </RelayFrame>
    );
  }

  if (!session?.authenticated) {
    const searchParams = new URLSearchParams(location.search);
    return (
      <RelayFrame>
        <RelayAuthPanel
          initialError={error}
          oauthError={searchParams.get('oauthError')}
          oauthNotice={
            searchParams.has('oauthPending')
              ? translate("auth.oAuthRegistrationReceivedAnAdminMustApprove")
              : null
          }
          onAuthenticated={handleAuthenticated}
          onRetry={error ? load : undefined}
          registrationEnabled={session?.registrationEnabled ?? false}
          settings={session?.registrationSettings}
        />
      </RelayFrame>
    );
  }

  return (
    <RelayFrame>
      <div
        aria-live="polite"
        className="w-full max-w-md py-12 text-sm text-[var(--theme-fg-muted)]"
        role="status"
      >
        {translate("auth.openingYourRelayWorkspace")}</div>
    </RelayFrame>
  );
}

function RelayAuthPanel({
  registrationEnabled,
  settings,
  oauthNotice,
  oauthError,
  initialError,
  onAuthenticated,
  onRetry,
}: {
  registrationEnabled: boolean;
  settings: RelaySessionDto['registrationSettings'];
  oauthNotice: string | null;
  oauthError: string | null;
  initialError: string | null;
  onAuthenticated: () => Promise<void>;
  onRetry: (() => Promise<void>) | undefined;
}) {
  const { locale: i18nLocale } = useI18n();
  const loginTabId = useId();
  const registerTabId = useId();
  const panelId = useId();
  const [mode, setMode] = useState<AuthMode>('login');
  const [challenge, setChallenge] = usePendingLogin();
  const [identifier, setIdentifier] = useState('');
  const [email, setEmail] = useState('');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [prepared, setPrepared] = useState<PreparedRegistration>();
  const [recoveryNewPassword, setRecoveryNewPassword] = useState('');
  const [registrationPassword, setRegistrationPassword] = useState('');
  const [error, setError] = useState(initialError ?? oauthError);
  const [notice, setNotice] = useState<string | null>(oauthNotice);
  const [submitting, setSubmitting] = useState(false);
  const [upgradeWarning, setUpgradeWarning] = useState<boolean>();
  const [upgradeConfirmed, setUpgradeConfirmed] = useState(false);
  const registrationPasswordRequired =
    settings?.registrationPasswordConfigured ??
    Boolean(settings?.registrationPassword);

  useEffect(() => {
    setError(initialError ?? oauthError);
  }, [initialError, oauthError]);
  useEffect(() => {
    setUpgradeConfirmed(false); setUpgradeWarning(undefined);
    if (mode !== 'upgrade') return;
    let active = true;
    void hasOpenedAccountVault().then(value => { if (active) setUpgradeWarning(value); })
      .catch(caught => { if (active) setError(errorMessage(caught, translate('auth.accountRequestFailed'))); });
    return () => { active = false; };
  }, [mode]);

  function selectMode(nextMode: AuthMode) {
    if (nextMode === 'register' && !registrationEnabled) {
      return;
    }
    setMode(nextMode);
    setError(null);
  }

  function handleModeKeyDown(
    event: ReactKeyboardEvent<HTMLButtonElement>,
    currentMode: AuthMode,
  ) {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) {
      return;
    }
    event.preventDefault();
    const nextMode: AuthMode =
      event.key === 'Home'
        ? 'login'
        : event.key === 'End'
          ? registrationEnabled
            ? 'register'
            : 'login'
          : currentMode === 'login' && registrationEnabled
            ? 'register'
            : 'login';
    selectMode(nextMode);
    window.requestAnimationFrame(() => {
      document
        .getElementById(nextMode === 'login' ? loginTabId : registerTabId)
        ?.focus();
    });
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    setNotice(null);
    try {
      if (mode === 'recover') {
        const result = await startAccountRecovery(identifier, password);
        if (result.challengeRequired) { setChallenge({ challengeRequired: true, authenticator: Boolean(result.authenticator), passkey: Boolean(result.passkey) }); return; }
      } else if (mode === 'reset') {
        const result = await startTemporaryResetLogin(identifier, password); setPassword('');
        if (result.challengeRequired) { setChallenge({ challengeRequired: true, authenticator: Boolean(result.authenticator), passkey: Boolean(result.passkey) }); return; }
      } else if (mode === 'upgrade') {
        const result = await startLegacyUpgrade(identifier, password, upgradeConfirmed); setPassword('');
        if (result.challengeRequired) { setChallenge({ challengeRequired: true, authenticator: Boolean(result.authenticator), passkey: Boolean(result.passkey) }); return; }
      } else if (mode === 'login') {
        const result = await relayLogin({ identifier, password });
        setPassword('');
        if (result.challengeRequired) {
          setChallenge({ challengeRequired: true, authenticator: Boolean(result.authenticator), passkey: Boolean(result.passkey) });
          return;
        }
      } else {
        if (!prepared) {
          if (username.trim().length < 3) throw new Error(translate('auth.usernameMustBeAtLeast3Characters'));
          setPrepared(await createVault(crypto.randomUUID(), password));
          return;
        }
        const code = registrationPassword.trim();
        const result = await registerAccount({ email, username, ...(code ? { registrationPassword: code } : {}) }, prepared);
        if (!result.pendingApproval) { await openVault(prepared.vault, password); await registerBrowser(); }
        if (result.pendingApproval) {
          setPrepared(undefined);
          setNotice(
            translate("auth.registrationRequestSentAnAdminMustApprove"),
          );
          setMode('login');
          return;
        }
      }
      await completeAuthentication();
    } catch (caught) {
      if ((caught as { code?: string }).code === 'user_id_taken') setPrepared(undefined);
      const message = errorMessage(caught, translate("auth.unableToAuthenticateWithTheRelay"));
      // A not-yet-secured account cannot sign in with a derived key; point to the explicit upgrade.
      setError(mode === 'login' && caught instanceof AccountTrustError && await legacyUpgradeMayApply(identifier)
        ? `${message} ${translate('auth.accountLoginLegacyHint')}` : message);
    } finally {
      setSubmitting(false);
    }
  }

  async function completeAuthentication() {
    if (mode === 'recover') {
      const session = await fetchRelaySession();
      if (!session.user) throw new Error(translate('auth.accountUnlockRequired'));
      await finishAccountLogin(session.user.id, session.user);
      await changeAccountPassword(recoveryNewPassword, await isOAuthAccount());
      await registerBrowser();
      setPassword(''); setRecoveryNewPassword('');
    }
    await onAuthenticated();
  }
  if (prepared) return <section className="w-full max-w-md space-y-4 rounded-xl border p-5">
    <RecoveryConfirmation code={prepared.recoveryCode} busy={submitting} onConfirm={() => {
      void submit({ preventDefault() {} } as FormEvent<HTMLFormElement>);
    }} />
    {error && <p role="alert">{error}</p>}
    <button disabled={submitting} onClick={() => setPrepared(undefined)}>{translate('auth.cancel')}</button>
  </section>;
  if (challenge) return <LoginVerification challenge={challenge} onSuccess={completeAuthentication} onBack={() => { cancelAccountLogin(); setChallenge(null); }} />;

  return (
    <section className="w-full max-w-md rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-5 shadow-[var(--theme-shadow)] sm:p-6">
      <LanguageSwitcher className="mb-4" />
      <p className="text-sm font-medium text-[var(--theme-accent-strong)]">
        {translate("auth.relayAccess")}</p>
      <h1 className="mt-2 text-2xl font-semibold text-[var(--theme-fg)]">
        {mode === 'login' ? translate("auth.welcomeBack") : translate("auth.createYourAccount")}
      </h1>
      <p className="mt-2 text-sm leading-6 text-[var(--theme-fg-muted)]">
        {mode === 'login'
          ? translate("auth.signInToOpenYourDevicesAnd")
          : translate("auth.createARelayUserAccountForPrivate")}
      </p>

      <div
        aria-label={translate("auth.accountAccess")}
        className="mt-5 grid grid-cols-2 rounded-lg bg-[var(--theme-muted)] p-1"
        role="tablist"
      >
        <button
          aria-controls={panelId}
          aria-selected={mode === 'login'}
          className={`min-h-11 rounded-md px-3 text-sm font-medium transition focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)] ${
            mode === 'login'
              ? 'bg-[var(--theme-panel)] text-[var(--theme-fg)] shadow-sm'
              : 'text-[var(--theme-fg-muted)] hover:text-[var(--theme-fg)]'
          }`}
          id={loginTabId}
          disabled={submitting}
          onKeyDown={(event) => handleModeKeyDown(event, 'login')}
          onClick={() => selectMode('login')}
          role="tab"
          tabIndex={mode === 'login' ? 0 : -1}
          type="button"
        >
          {translate("auth.signIn")}</button>
        <button
          aria-controls={panelId}
          aria-selected={mode === 'register'}
          className={`min-h-11 rounded-md px-3 text-sm font-medium transition focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)] ${
            mode === 'register'
              ? 'bg-[var(--theme-panel)] text-[var(--theme-fg)] shadow-sm'
              : 'text-[var(--theme-fg-muted)] hover:text-[var(--theme-fg)]'
          }`}
          disabled={!registrationEnabled || submitting}
          id={registerTabId}
          onKeyDown={(event) => handleModeKeyDown(event, 'register')}
          onClick={() => selectMode('register')}
          role="tab"
          tabIndex={mode === 'register' ? 0 : -1}
          title={registrationEnabled ? undefined : translate("auth.registrationIsDisabled")}
          type="button"
        >
          {registrationEnabled ? translate("auth.createAccount") : translate("auth.registrationClosed")}
        </button>
      </div>

      {settings?.googleAuthEnabled || settings?.githubAuthEnabled ? (
        <div className="mt-5 grid gap-2">
          {settings.googleAuthEnabled ? (
            <a
              className="relay-button-secondary flex h-11 items-center justify-center"
              href={relayUrl("/relay/auth/oauth/google/start")}
            >
              {translate("auth.continueWithGoogle")}</a>
          ) : null}
          {settings.githubAuthEnabled ? (
            <a
              className="relay-button-secondary flex h-11 items-center justify-center"
              href={relayUrl("/relay/auth/oauth/github/start")}
            >
              {translate("auth.continueWithGitHub")}</a>
          ) : null}
          <div className="flex items-center gap-3 py-1 text-xs text-[var(--theme-fg-muted)]">
            <span className="h-px flex-1 bg-[var(--theme-border)]" />
            <span>{translate("auth.orUseAPassword")}</span>
            <span className="h-px flex-1 bg-[var(--theme-border)]" />
          </div>
        </div>
      ) : null}

      <form
        aria-labelledby={mode === 'login' ? loginTabId : registerTabId}
        className="mt-5 space-y-4"
        id={panelId}
        onSubmit={submit}
        role="tabpanel"
      >
        {mode !== 'register' ? (
          <RelayInput
            autoComplete="username"
            disabled={submitting}
            label={translate("auth.emailOrUsername")}
            name="identifier"
            onChange={setIdentifier}
            required
            value={identifier}
          />
        ) : (
          <>
            <RelayInput
              autoComplete="email"
              disabled={submitting}
              label={translate("auth.email")}
              name="email"
              onChange={setEmail}
              required
              type="email"
              value={email}
            />
            <RelayInput
              autoComplete="username"
              disabled={submitting}
              label={translate("auth.username")}
              minLength={3}
              name="username"
              onChange={setUsername}
              required
              value={username}
            />
            <RelayInput
              autoComplete="one-time-code"
              disabled={submitting}
              description={
                registrationPasswordRequired
                  ? translate("auth.requiredByThisRelay")
                  : translate("auth.enterTheInviteCodeIfThisRelay")
              }
              label={
                registrationPasswordRequired
                  ? translate("auth.registrationCode")
                  : translate("auth.registrationCodeIfRequired")
              }
              name="registrationCode"
              onChange={setRegistrationPassword}
              required={registrationPasswordRequired}
              type="password"
              value={registrationPassword}
            />
          </>
        )}
        <RelayInput
          autoComplete={mode === 'login' ? 'current-password' : mode === 'recover' || mode === 'reset' ? 'off' : 'new-password'}
          disabled={submitting}
          label={translate(mode === 'recover' ? 'auth.accountRecoveryCode' : mode === 'reset' ? 'auth.accountTemporaryPassword' : 'auth.password')}
          name="password"
          onChange={setPassword}
          required
          type={mode === 'recover' ? 'text' : 'password'}
          value={password}
          {...(mode === 'register'
            ? { description: translate("auth.accountPasswordHelp"), minLength: 12 }
            : {})}
        />

        {mode === 'recover' && <RelayInput autoComplete="new-password" label={translate('auth.newPassword')} name="newPassword" type="password" minLength={12} required value={recoveryNewPassword} onChange={setRecoveryNewPassword} description={translate('auth.accountPasswordHelp')} />}
        {mode === 'recover' && <p className="text-sm text-[var(--theme-fg-muted)]">{translate('auth.accountSecretRotationWarning')}</p>}
        {mode !== 'register' && <button type="button" className="owner-trust-account-link" onClick={() => { setMode(mode === 'recover' ? 'login' : 'recover'); setPassword(''); }}>{translate(mode === 'recover' ? 'auth.accountUsePassword' : 'auth.accountUseRecovery')}</button>}
        {mode !== 'register' && <button type="button" className="owner-trust-account-link" onClick={() => { setMode(mode === 'reset' ? 'login' : 'reset'); setPassword(''); }}>{translate(mode === 'reset' ? 'auth.accountUsePassword' : 'auth.accountTemporaryLogin')}</button>}
        {mode !== 'register' && legacyUpgradeEnabled() && <button type="button" className="owner-trust-account-link" onClick={() => { setMode(mode === 'upgrade' ? 'login' : 'upgrade'); setPassword(''); }}>{translate(mode === 'upgrade' ? 'auth.accountUsePassword' : 'auth.accountLegacyUpgrade')}</button>}
        {mode === 'upgrade' && <p className="text-sm">{translate('auth.accountLegacyUpgradeHelp')}</p>}
        {mode === 'upgrade' && upgradeWarning && <div className="space-y-3 rounded border p-3">
          <p role="alert">{translate('auth.accountUpgradeExistingVaultWarning')}</p>
          <label className="flex items-start gap-2"><input type="checkbox" checked={upgradeConfirmed} onChange={event => setUpgradeConfirmed(event.target.checked)} disabled={submitting} />{translate('auth.accountUpgradeExistingVaultConfirm')}</label>
        </div>}
        {mode === 'reset' && <p className="text-sm">{translate('auth.accountTemporaryHelp')}</p>}
        {error ? (
          <RelayNotice tone="danger">
            <p>{error}</p>
            {initialError && onRetry ? (
              <button
                className="relay-button-secondary mt-3 h-11"
                onClick={() => void onRetry()}
                type="button"
              >
                {translate("auth.retryConnection")}</button>
            ) : null}
          </RelayNotice>
        ) : null}
        {notice ? <RelayNotice tone="accent">{notice}</RelayNotice> : null}

        <button
          className="relay-button-primary h-11 w-full"
          disabled={submitting || (mode === 'upgrade' && (upgradeWarning === undefined || (upgradeWarning && !upgradeConfirmed)))}
          type="submit"
        >
          {submitting
            ? translate("auth.working")
            : mode === 'login' || mode === 'reset' || mode === 'upgrade'
              ? translate("auth.signIn")
              : translate(mode === 'recover' ? 'auth.accountRecover' : "auth.createAccount")}
        </button>
      </form>
    </section>
  );
}

function RelayFrame({ children }: { children: React.ReactNode }) {
  const { locale: i18nLocale } = useI18n();
  return (
    <main className="min-h-screen bg-[var(--app-bg)] px-4 py-5 text-[var(--app-fg)] sm:px-6 sm:py-6" data-page="relay-portal">
      <header className="mx-auto flex w-full max-w-5xl items-center justify-between gap-3 border-b border-[var(--theme-border)] pb-4">
        <Link
          className="flex min-h-11 min-w-0 items-center gap-2 text-sm font-semibold text-[var(--theme-fg)]"
          to="/"
        >
          <ArrowLeft aria-hidden="true" className="h-4 w-4 shrink-0" />
          <img src="/icon-192.png?v=pockymoe" alt="" className="h-8 w-8 shrink-0 rounded-lg" />
          <span className="truncate">{translate("auth.relayHome")}</span>
        </Link>
        <Link
          className="relay-button-secondary inline-flex h-11 shrink-0 items-center gap-2"
          to="/relay-guide"
        >
          <BookOpen aria-hidden="true" className="h-4 w-4" />
          {translate("auth.guide")}</Link>
      </header>
      <div className="mx-auto flex w-full max-w-5xl justify-center py-8 sm:py-12">
        {children}
      </div>
    </main>
  );
}

function RelayInput({
  label,
  value,
  onChange,
  name,
  type = 'text',
  autoComplete,
  description,
  disabled = false,
  minLength,
  required = false,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  name: string;
  type?: string;
  autoComplete?: string;
  description?: string;
  disabled?: boolean;
  minLength?: number;
  required?: boolean;
}) {
  const { locale: i18nLocale } = useI18n();
  const descriptionId = useId();
  const inputId = useId();
  const [passwordVisible, setPasswordVisible] = useState(false);
  const passwordField = type === 'password';
  return (
    <div className="text-sm text-[var(--theme-fg-soft)]">
      <label htmlFor={inputId}>{label}</label>
      <div className="relative mt-2">
        <input
          aria-describedby={description ? descriptionId : undefined}
          autoComplete={autoComplete}
          className={`relay-input min-h-11 w-full ${passwordField ? 'pr-12' : ''}`}
          disabled={disabled}
          id={inputId}
          minLength={minLength}
          name={name}
          onChange={(event) => onChange(event.target.value)}
          required={required}
          type={passwordField && passwordVisible ? 'text' : type}
          value={value}
        />
        {passwordField ? (
          <button
            aria-label={
              passwordVisible
                ? translate("auth.hide", { value1: label.toLowerCase() })
                : translate("auth.show", { value1: label.toLowerCase() })
            }
            className="absolute inset-y-0 right-0 inline-flex w-11 items-center justify-center text-[var(--theme-fg-muted)] hover:text-[var(--theme-fg)]"
            disabled={disabled}
            onClick={() => setPasswordVisible((visible) => !visible)}
            type="button"
          >
            {passwordVisible ? (
              <EyeOff aria-hidden="true" className="h-4 w-4" />
            ) : (
              <Eye aria-hidden="true" className="h-4 w-4" />
            )}
          </button>
        ) : null}
      </div>
      {description ? (
        <span
          className="mt-1.5 block text-xs text-[var(--theme-fg-muted)]"
          id={descriptionId}
        >
          {description}
        </span>
      ) : null}
    </div>
  );
}

function RelayNotice({
  tone,
  children,
}: {
  tone: 'accent' | 'danger';
  children: React.ReactNode;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <div
      aria-live={tone === 'danger' ? 'assertive' : 'polite'}
      className={`rounded-lg px-3 py-2 text-sm ${
        tone === 'danger'
          ? 'bg-[var(--status-danger-bg)] text-[var(--status-danger-fg)]'
          : 'bg-[var(--theme-accent-soft)] text-[var(--theme-fg)]'
      }`}
      role={tone === 'danger' ? 'alert' : 'status'}
    >
      {children}
    </div>
  );
}
