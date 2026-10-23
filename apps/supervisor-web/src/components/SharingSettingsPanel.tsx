import { ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useId, useState } from 'react';
import { fetchSharingPreferences, saveRequireSafetyCode } from '../lib/sharingPreferences';

/** Account settings → Sharing: whether people I share with must have their
    safety code confirmed before access starts (saved to the account,
    docs/account-key-trust.md#safety-code-check). My own code is in the account
    key section below. */
export function SharingSettingsPanel() {
  useI18n();
  const hintId = useId();
  const [required, setRequired] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<{ at: 'load' | 'save'; message: string } | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let active = true;
    void fetchSharingPreferences(true)
      .then(preferences => { if (active) { setRequired(preferences.requireSafetyCode); setError(null); } })
      .catch((caught: unknown) => {
        if (active) setError({ at: 'load', message: caught instanceof Error ? caught.message : translate('sharing.settingLoadFailed') });
      });
    return () => { active = false; };
  }, [attempt]);
  useEffect(() => {
    if (!saved) return;
    const timer = window.setTimeout(() => setSaved(false), 2000);
    return () => window.clearTimeout(timer);
  }, [saved]);
  function change(next: boolean) {
    setSaving(true);
    setSaved(false);
    setError(null);
    setRequired(next);
    void saveRequireSafetyCode(next)
      .then(preferences => { setRequired(preferences.requireSafetyCode); setSaved(true); })
      .catch((caught: unknown) => {
        setRequired(!next);
        setError({ at: 'save', message: caught instanceof Error ? caught.message : translate('sharing.settingSaveFailed') });
      })
      .finally(() => setSaving(false));
  }
  return (
    <section className="grid gap-5 py-6 sm:grid-cols-[11rem_minmax(0,1fr)] sm:gap-8" aria-labelledby="sharing-settings-heading">
      <header>
        <h2 id="sharing-settings-heading" className="text-base font-semibold text-[var(--theme-fg)]">{translate('sharing.settingsTitle')}</h2>
        <p className="mt-1 text-sm leading-5 text-[var(--theme-fg-muted)]">{translate('sharing.settingsDescription')}</p>
      </header>
      <div className="min-w-0 max-w-md space-y-5">
        <div className="space-y-2">
          <div className="flex items-start gap-3 py-1">
            <input
              id="require-safety-code"
              type="checkbox"
              className="mt-1 h-4 w-4 shrink-0 accent-[var(--theme-accent-solid)]"
              checked={required ?? true}
              disabled={required === null || saving}
              aria-describedby={hintId}
              onChange={event => change(event.target.checked)}
            />
            <label htmlFor="require-safety-code" className="text-sm font-medium text-[var(--theme-fg)]">
              {translate('sharing.requireCodeLabel')}
            </label>
            {saved ? <span className="ml-auto shrink-0 text-xs text-[var(--status-success-fg)]" role="status">{translate('sharing.settingSaved')}</span> : null}
            {error ? <ErrorMark message={error.message} onRetry={error.at === 'load' ? () => setAttempt(value => value + 1) : undefined} /> : null}
          </div>
          <p id={hintId} className="pl-7 text-xs leading-5 text-[var(--theme-fg-muted)]">
            {translate('sharing.requireCodeHint')}{' '}
            {required === false ? translate('sharing.requireCodeOffHint') : null}
          </p>
        </div>
      </div>
    </section>
  );
}
