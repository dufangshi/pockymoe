// /relay-admin: signs the relay admin in (password, then a second factor when
// the admin has one) and shows the admin console (pages/relayAdmin).
import { ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import type { RelaySessionDto } from '@pockymoe/shared';
import { LoginVerification, usePendingLogin } from '../components/RelaySecurity';
import { ApiError, enableRelayMode, fetchRelayAdminSession, relayAdminLogin, relayAdminLogout } from '../lib/api';
import { adminApi } from './relayAdmin/adminApi';
import { RelayAdminConsole } from './relayAdmin/RelayAdminConsole';
import { LoginPage } from './LoginPage';

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

export function RelayAdminPage() {
  useI18n();
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
      <LoginPage
        description={translate('admin.signInDescription')}
        eyebrow={translate('admin.consoleTitle')}
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
