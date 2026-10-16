import { appUrl } from '../lib/webBase';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { LanguageSwitcher } from '@pockymoe/thread-ui/i18n';
import { FormEvent, useState } from 'react';
import { Eye, EyeOff } from 'lucide-react';

import { ApiError } from '../lib/api';

export function LoginPage({
  eyebrow = translate("auth.supervisorAccess"),
  description,
  passcode = false,
  onLogin,
}: {
  eyebrow?: string;
  description?: string;
  /** Local mode: the passcode alone, no username. */
  passcode?: boolean;
  onLogin: (input: { username: string; password: string }) => Promise<void>;
}) {
  useI18n();
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [passwordVisible, setPasswordVisible] = useState(false);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      await onLogin({
        username,
        password,
      });
    } catch (caught) {
      if (caught instanceof ApiError && passcode && caught.statusCode === 401) {
        setError(translate("auth.passcodeWrong"));
      } else if (caught instanceof ApiError && passcode && caught.statusCode === 429) {
        setError(translate("auth.passcodeRateLimited"));
      } else if (caught instanceof ApiError) {
        setError(caught.payload.message);
      } else {
        setError(translate("auth.unableToSignIn"));
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <main className="flex min-h-screen items-center justify-center bg-[var(--app-bg)] px-4 py-8 text-[var(--app-fg)]" data-page="login">
      <section className="w-full max-w-sm rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-5 shadow-[var(--theme-shadow)] sm:p-6">
        <div className="mb-5"><LanguageSwitcher className="mb-4" />
          <div className="mb-5 flex items-center gap-3 border-b border-[var(--theme-border)] pb-4">
            <img src={appUrl('icon-192.png?v=pockymoe')} alt="" className="h-10 w-10 rounded-md" />
            <div>
              <p className="text-sm font-semibold text-[var(--theme-fg)]">Pockymoe</p>
              <p className="text-xs text-[var(--theme-fg-muted)]">{eyebrow}</p>
            </div>
          </div>
          <h1 className="mt-2 text-2xl font-semibold tracking-normal text-[var(--theme-fg)]">
            {translate(passcode ? "auth.passcodeTitle" : "auth.signIn")}</h1>
          <p className="mt-2 text-sm leading-6 text-[var(--theme-fg-muted)]">
            {description ?? translate(passcode ? "auth.passcodeDescription" : "auth.adminCredentialDescription")}
          </p>
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          {!passcode && <label className="block">
            <span className="text-sm font-medium text-[var(--theme-fg-soft)]">
              {translate("auth.username")}</span>
            <input
              autoComplete="username"
              autoFocus
              className="host-form-control mt-2 h-11 w-full rounded-md border px-3 text-sm outline-none transition"
              disabled={submitting}
              name="username"
              onChange={(event) => {
                setUsername(event.target.value);
                setError(null);
              }}
              value={username}
            />
          </label>}

          <label className="block">
            <span className="text-sm font-medium text-[var(--theme-fg-soft)]">
              {translate(passcode ? "auth.passcodeLabel" : "auth.password")}</span>
            <span className="relative mt-2 block">
              <input
                autoComplete="current-password"
                autoFocus={passcode}
                className="host-form-control h-11 w-full rounded-md border px-3 pr-12 text-sm outline-none transition"
                disabled={submitting}
                name="password"
                onChange={(event) => {
                  setPassword(event.target.value);
                  setError(null);
                }}
                type={passwordVisible ? 'text' : 'password'}
                value={password}
              />
              <button
                aria-label={passwordVisible ? translate("auth.hidePassword") : translate("auth.showPassword")}
                className="absolute inset-y-0 right-0 inline-flex w-11 items-center justify-center text-[var(--theme-fg-muted)] hover:text-[var(--theme-fg)]"
                disabled={submitting}
                onClick={() => setPasswordVisible((visible) => !visible)}
                type="button"
              >
                {passwordVisible ? <EyeOff aria-hidden="true" className="h-4 w-4" /> : <Eye aria-hidden="true" className="h-4 w-4" />}
              </button>
            </span>
          </label>

          {error && (
            <p className="rounded-md border border-[var(--status-danger-border)] bg-[var(--status-danger-bg)] px-3 py-2 text-sm text-[var(--status-danger-fg)]" role="alert">
              {error}
            </p>
          )}

          <button
            className="ui-action-primary h-11 w-full rounded-md px-4 text-sm font-semibold transition disabled:cursor-not-allowed"
            disabled={submitting || (!passcode && !username.trim()) || !password}
            type="submit"
          >
            {submitting ? translate("auth.signingIn") : translate("auth.signIn")}
          </button>
        </form>
      </section>
    </main>
  );
}
