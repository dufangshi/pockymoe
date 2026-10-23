// /relay-admin: signs the relay admin in (password, then a second factor when
// the admin has one) and shows the admin console (pages/relayAdmin). The
// console always shows Classic, whatever theme pack the browser chose: one
// fixed look for an operations tool. The choice itself is untouched and comes
// back on leaving (docs/theme-packs.md, docs/ui-principles.md).
import { ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ShieldCheck } from 'lucide-react';
import { useCallback, useEffect, useLayoutEffect, useState, type FormEvent } from 'react';
import { Link } from 'react-router-dom';
import type { RelaySessionDto } from '@pockymoe/shared';
import { AuthCard, AuthField, AuthShell, AuthSubmit } from '../components/auth/AuthUi';
import { LoginVerification, usePendingLogin } from '../components/RelaySecurity';
import { ApiError, enableRelayMode, fetchRelayAdminSession, relayAdminLogin, relayAdminLogout } from '../lib/api';
import { CLASSIC_THEME_ID, pinTheme } from '../lib/themePacks';
import { adminApi } from './relayAdmin/adminApi';
import { RelayAdminConsole } from './relayAdmin/RelayAdminConsole';

type State =
  | { kind: 'loading' }
  | { kind: 'login' }
  | { kind: 'unsupported'; status: number }
  | { kind: 'failed'; message: string }
  | { kind: 'ready'; session: RelaySessionDto };

function errorMessage(caught: unknown) {
  return caught instanceof ApiError
    ? caught.payload.message
    : caught instanceof Error
      ? caught.message
      : translate('admin.requestFailed');
}

/** The console's own sign-in: the administrator's name and password, with a
    way to the user sign-in for everyone else. */
function AdminSignIn({ onLogin }: { onLogin: (input: { username: string; password: string }) => Promise<void> }) {
  useI18n();
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await onLogin({ username: username.trim(), password });
      setPassword('');
    } catch (caught) {
      setError(caught instanceof ApiError && caught.statusCode === 401 ? translate('admin.wrongAdminCredentials') : errorMessage(caught));
    } finally {
      setBusy(false);
    }
  };
  return (
    <AuthShell>
      <AuthCard
        footer={
          <span>
            {translate('admin.notAdministrator')}{' '}
            <Link className="font-medium text-[var(--theme-accent-strong)] hover:underline" to="/relay-portal">{translate('admin.userSignIn')}</Link>
          </span>
        }
        icon={<ShieldCheck aria-hidden className="h-7 w-7" />}
        subtitle={translate('admin.signInDescription')}
        title={translate('admin.signInTitle')}
      >
        <form className="space-y-4" onSubmit={(event) => void submit(event)}>
          <AuthField autoComplete="username" autoFocus disabled={busy} label={translate('auth.username')} name="username" onChange={setUsername} spellCheck={false} value={username} />
          <AuthField autoComplete="current-password" disabled={busy} label={translate('auth.password')} name="password" onChange={setPassword} type="password" value={password} />
          <AuthSubmit busy={busy} disabled={!username.trim() || !password} error={error}>{translate('auth.signIn')}</AuthSubmit>
        </form>
      </AuthCard>
    </AuthShell>
  );
}

export function RelayAdminPage() {
  useI18n();
  // Classic while the console (or its sign-in) is shown; the stored theme
  // returns when it unmounts. A layout effect, so a pack never paints first.
  useLayoutEffect(() => pinTheme(CLASSIC_THEME_ID), []);
  const [state, setState] = useState<State>({ kind: 'loading' });
  const [challenge, setChallenge] = usePendingLogin();

  /** Checks the admin session and that this relay has the console API. */
  const open = useCallback(async (known?: RelaySessionDto) => {
    try {
      enableRelayMode();
      const session = known ?? (await fetchRelayAdminSession());
      if (!session.authenticated || session.user?.role !== 'admin') {
        setState({ kind: 'login' });
        return;
      }
      try {
        await adminApi.registrationSettings();
      } catch (caught) {
        // An older relay has only the single summary route.
        if (caught instanceof ApiError && [404, 405, 501].includes(caught.statusCode)) {
          setState({ kind: 'unsupported', status: caught.statusCode });
          return;
        }
        throw caught;
      }
      setState({ kind: 'ready', session });
    } catch (caught) {
      if (caught instanceof ApiError && (caught.statusCode === 401 || caught.statusCode === 403)) setState({ kind: 'login' });
      else setState({ kind: 'failed', message: errorMessage(caught) });
    }
  }, []);

  useEffect(() => {
    void open();
  }, [open]);

  const logout = useCallback(async () => {
    try {
      await relayAdminLogout();
    } catch {
      // The session is gone either way; show the sign-in.
    }
    setState({ kind: 'login' });
  }, []);

  if (challenge) {
    return (
      <main className="flex min-h-screen items-center justify-center p-4" data-page="admin-verify">
        <LoginVerification
          challenge={challenge}
          onBack={() => setChallenge(null)}
          onSuccess={async () => {
            setChallenge(null);
            await open();
          }}
        />
      </main>
    );
  }

  if (state.kind === 'login') {
    return (
      <AdminSignIn
        onLogin={async (input) => {
          const result = await relayAdminLogin(input);
          if (result.challengeRequired) {
            setChallenge({ challengeRequired: true, authenticator: Boolean(result.authenticator), passkey: Boolean(result.passkey) });
            return;
          }
          if (!result.session.authenticated || result.session.user?.role !== 'admin') {
            throw new Error(translate('admin.notAnAdmin'));
          }
          await open(result.session);
        }}
      />
    );
  }

  if (state.kind === 'ready') {
    return <RelayAdminConsole onLogout={() => void logout()} onUnauthorized={() => setState({ kind: 'login' })} session={state.session} />;
  }

  return (
    <main className="min-h-screen bg-[var(--app-bg)] px-4 pt-[env(safe-area-inset-top)] text-[var(--app-fg)] sm:px-6" data-page="admin">
      <div className="product-page flex flex-col gap-5 py-10">
        {state.kind === 'loading' ? (
          <section aria-busy="true" aria-label={translate('admin.loading')} className="product-panel">
            <div className="product-row min-h-24">
              <div className="min-w-0 flex-1 space-y-3">
                <div className="product-skeleton h-4 w-40" />
                <div className="product-skeleton h-3 w-72 max-w-full" />
              </div>
            </div>
          </section>
        ) : (
          <section className="product-panel" role="status">
            <div className="p-5 sm:p-6">
              <div className="flex flex-wrap items-center gap-2">
                <h1 className="text-base font-semibold text-[var(--theme-fg)]">
                  {state.kind === 'unsupported' ? translate('admin.apiMissing') : translate('admin.couldNotLoad')}
                </h1>
                {state.kind === 'unsupported' ? (
                  <>
                    <span className="rounded-full border border-[var(--status-warning-border)] bg-[var(--status-warning-bg)] px-2 py-0.5 text-xs font-medium text-[var(--status-warning-fg)]">
                      HTTP {state.status}
                    </span>
                    <ErrorMark kind="unavailable" message={translate('admin.apiMissingHelp')} />
                  </>
                ) : (
                  <ErrorMark message={state.message} />
                )}
              </div>
              <div className="mt-5 flex flex-wrap gap-2">
                <button className="relay-button-primary" onClick={() => void open()} type="button">
                  {translate('admin.retry')}
                </button>
                <Link className="relay-button-secondary" to="/">
                  {translate('admin.relayHome')}
                </Link>
              </div>
            </div>
          </section>
        )}
      </div>
    </main>
  );
}
