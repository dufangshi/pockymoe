import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ErrorMark } from '@pockymoe/thread-ui';
import { ShieldCheck } from 'lucide-react';
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { setupAccount } from '../../lib/accountAuth';
import { accountStore } from '../../lib/accountStore';
import { AccountTrustError, createVault, openVault, registerBrowser } from '../../lib/accountVaultState';
import { AuthCard, AuthField, AuthSubmit, RecoveryCodeStep } from './AuthUi';

const longEnough = (password: string) => [...password.normalize('NFC')].length >= 12;

/**
 * Creates the account vault for an account that has none yet: a legacy password
 * account after its upgrade sign-in, or a Google/GitHub account. The vault is
 * stored before the recovery code is shown, so the code is never displayed for a
 * vault that failed to save.
 */
export function AccountSetupFlow({ oauth, initialPassword, footer, onDone, onReauthenticate }: {
  oauth: boolean;
  /** The password the user just signed in with; reused when it is long enough. */
  initialPassword?: string | undefined;
  footer?: ReactNode;
  onDone: () => void;
  onReauthenticate: () => void;
}) {
  useI18n();
  const reuse = !oauth && !!initialPassword && longEnough(initialPassword);
  const [password, setPassword] = useState(''), [confirm, setConfirm] = useState('');
  const [busy, setBusy] = useState(reuse), [error, setError] = useState<string>();
  const [recoveryCode, setRecoveryCode] = useState<string>();
  // Steps already done survive a failed later step, so a retry never creates a
  // second vault (the relay would refuse it) and reuses the same secret.
  const progress = useRef<{ secret?: string; prepared?: Awaited<ReturnType<typeof createVault>>; stored?: boolean }>({});
  const [stored, setStored] = useState(false);
  const started = useRef(false);

  async function secure(secret: string) {
    setBusy(true); setError(undefined);
    const step = progress.current;
    try {
      const userId = await accountStore<string>('active');
      if (!userId) throw new AccountTrustError('accountUnlockRequired');
      if (!step.prepared) { step.secret = secret; step.prepared = await createVault(userId, secret); }
      if (!step.stored) { await setupAccount(step.prepared, oauth); step.stored = true; setStored(true); }
      await openVault(step.prepared.vault, step.secret!);
      await registerBrowser(undefined, 'account-setup');
      setRecoveryCode(step.prepared.recoveryCode);
    } catch (caught) {
      if (!step.stored) progress.current = {};
      if (caught instanceof AccountTrustError && caught.code === 'reauthentication_required') { onReauthenticate(); return; }
      setError(caught instanceof Error ? caught.message : translate('auth.accountRequestFailed'));
    } finally { setBusy(false); }
  }

  useEffect(() => {
    if (reuse && !started.current) { started.current = true; void secure(initialPassword!); }
  }, []);

  if (recoveryCode) return <RecoveryCodeStep code={recoveryCode} onDone={onDone} />;

  function submit(event: FormEvent) {
    event.preventDefault();
    if (password !== confirm) { setError(translate('auth.ui.passwordMismatch')); return; }
    if (!longEnough(password)) { setError(translate('auth.accountPasswordShort')); return; }
    void secure(password);
  }

  if (stored && !recoveryCode) {
    // The vault is saved; only connecting this browser is left to retry.
    return (
      <AuthCard footer={footer} icon={<ShieldCheck aria-hidden="true" className="h-8 w-8" />} title={translate(oauth ? 'auth.ui.unlockPasswordTitle' : 'auth.ui.secureTitle')}>
        {busy ? <p aria-live="polite" className="text-sm text-[var(--theme-fg-muted)]" role="status">{translate('auth.ui.securing')}</p> : null}
        <div className="flex items-center gap-2">
          <button className="relay-button-primary auth-submit h-11 w-full min-w-0" disabled={busy} onClick={() => void secure(progress.current.secret!)} type="button">
            {translate(busy ? 'auth.ui.pleaseWait' : 'auth.retryConnection')}
          </button>
          {error ? <ErrorMark message={error} /> : null}
        </div>
      </AuthCard>
    );
  }
  if (reuse && busy && !error) {
    return (
      <AuthCard footer={footer} icon={<ShieldCheck aria-hidden="true" className="h-8 w-8" />} title={translate('auth.ui.secureTitle')}>
        <p aria-live="polite" className="text-sm text-[var(--theme-fg-muted)]" role="status">{translate('auth.ui.securing')}</p>
      </AuthCard>
    );
  }
  return (
    <AuthCard
      footer={footer}
      icon={<ShieldCheck aria-hidden="true" className="h-8 w-8" />}
      subtitle={translate(oauth ? 'auth.ui.unlockPasswordBody' : 'auth.ui.secureBody')}
      title={translate(oauth ? 'auth.ui.unlockPasswordTitle' : 'auth.ui.secureTitle')}
    >
      <form className="space-y-4" onSubmit={submit}>
        <AuthField autoComplete="new-password" autoFocus disabled={busy} hint={translate('auth.ui.passwordRule')}
          label={translate(oauth ? 'auth.ui.unlockPasswordLabel' : 'auth.password')} minLength={12} onChange={setPassword} type="password" value={password} />
        <AuthField autoComplete="new-password" disabled={busy} label={translate('auth.ui.confirmPassword')} onChange={setConfirm} type="password" value={confirm} />
        <AuthSubmit busy={busy} error={error}>{translate('auth.continue')}</AuthSubmit>
      </form>
    </AuthCard>
  );
}
