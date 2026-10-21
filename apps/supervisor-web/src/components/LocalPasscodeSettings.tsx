import type { AuthSessionDto } from '@pockymoe/shared';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { confirmAsync, ErrorMark } from '@pockymoe/thread-ui';
import { type FormEvent, useEffect, useState } from 'react';

import { ApiError, fetchAuthSession, relayModeActive, removeLocalPasscode, setLocalPasscode } from '../lib/api';

/**
 * The optional passcode of a Supervisor in local mode. It starts open; the owner
 * can require a passcode here so others on the network cannot just walk in.
 */
export function LocalPasscodeSettings() {
  useI18n();
  const [session, setSession] = useState<AuthSessionDto | null>(null);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // A too-short draft is a hint of the field being typed; a refusal is a mark.
  const [tooShort, setTooShort] = useState(false);

  useEffect(() => {
    if (relayModeActive()) return;
    let active = true;
    void fetchAuthSession().then(value => { if (active) setSession(value); }).catch(() => undefined);
    return () => { active = false; };
  }, []);

  if (!session || session.mode !== 'local') return null;
  const enabled = session.passcode === true;

  async function run(work: () => Promise<void>, done: string) {
    setBusy(true); setError(null); setTooShort(false); setMessage(null);
    try { await work(); setDraft(''); setMessage(done); }
    catch (caught) { setError(caught instanceof ApiError ? caught.payload.message : translate('auth.unableToSignIn')); }
    finally { setBusy(false); }
  }

  function save(event: FormEvent) {
    event.preventDefault();
    if ([...draft].length < 4) { setError(null); setTooShort(true); return; }
    void run(async () => {
      const result = await setLocalPasscode(draft);
      setSession(result.session);
    }, translate('settings.passcodeSaved'));
  }

  async function disable() {
    if (!(await confirmAsync({ title: translate('settings.passcodeDisableTitle'), description: translate('settings.passcodeDisableConfirm'), confirmLabel: translate('settings.passcodeDisableAction') }))) return;
    void run(async () => { setSession(await removeLocalPasscode()); }, translate('settings.passcodeRemoved'));
  }

  return (
    <section className="local-passcode-settings space-y-3 rounded-lg border border-[var(--theme-border)] p-4" aria-labelledby="local-passcode-title">
      <div>
        <h3 className="text-sm font-semibold text-[var(--theme-fg)]" id="local-passcode-title">{translate('settings.passcodeTitle')}</h3>
        <p className="mt-1 text-sm text-[var(--theme-fg-muted)]">
          {translate(enabled ? 'settings.passcodeOnDescription' : 'settings.passcodeOffDescription')}
        </p>
      </div>
      <form className="flex flex-wrap items-end gap-2" onSubmit={save}>
        <label className="min-w-[12rem] flex-1">
          <span className="text-xs font-medium text-[var(--theme-fg-soft)]">
            {translate(enabled ? 'settings.passcodeNewLabel' : 'settings.passcodeLabel')}
          </span>
          <input
            autoComplete="new-password"
            className="host-input mt-1 h-10 w-full rounded-md border px-3 text-sm"
            disabled={busy}
            onChange={event => { setDraft(event.target.value); setError(null); setTooShort(false); }}
            type="password"
            value={draft}
          />
        </label>
        <button className="relay-button-primary min-h-10 rounded-md px-4" disabled={busy || !draft} type="submit">
          {translate(enabled ? 'settings.passcodeChange' : 'settings.passcodeEnable')}
        </button>
        {enabled && (
          <button className="host-secondary-button min-h-10 rounded-md border px-3 text-xs font-medium transition disabled:cursor-not-allowed disabled:opacity-50" disabled={busy} onClick={disable} type="button">
            {translate('settings.passcodeDisable')}
          </button>
        )}
        {error && <span className="flex min-h-10 items-center"><ErrorMark message={error} /></span>}
      </form>
      {tooShort && <p className="text-sm text-[var(--status-danger-fg)]" role="alert">{translate('settings.passcodeTooShort')}</p>}
      {message && <p className="text-sm text-[var(--status-success-fg)]" role="status">{message}</p>}
      {enabled && <p className="text-xs text-[var(--theme-fg-muted)]">{translate('settings.passcodeForgot')}</p>}
    </section>
  );
}
