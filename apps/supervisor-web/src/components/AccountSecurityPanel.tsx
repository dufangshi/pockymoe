import { SecurityVerification } from './RelaySecurity';
import { securityRequest, type SecurityStatus } from '../lib/relaySecurity';
import { getLocale, translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useRef, useState } from 'react';
import { currentAccount, currentBrowser, safetyCode, type BrowserCertificate } from '../lib/accountVault';
import { accountRequest, accountRevocations, changeAccountPassword, regenerateRecoveryCode, revokeAccountSubject,
  startAccountOver } from '../lib/accountVaultState';
import { accountSessionState, isOAuthAccount } from '../lib/accountAuth';
import { verifyBrowserCertificate } from '../lib/accountCrypto';
import { FormDialog } from './FormDialog';
import { RecoveryConfirmation } from './AccountUnlockPrompt';
interface BrowserRow { certificate: BrowserCertificate; label: string; createdAt: number; lastSeenAt: number }
export function AccountSecurityPanel() {
  useI18n(); const [browsers, setBrowsers] = useState<BrowserRow[]>([]), [thisId, setThisId] = useState<string>();
  const [code, setCode] = useState(''), [error, setError] = useState<string>(), [busy, setBusy] = useState(false);
  const [action, setAction] = useState<'password' | 'reset' | 'recovery'>(), [password, setPassword] = useState('');
  const [recovery, setRecovery] = useState<string>(), [message, setMessage] = useState<string>();
  const [canStartOver, setCanStartOver] = useState(false);
  async function load() {
    setCanStartOver(!!(await accountSessionState())?.recovery);
    const account = await currentAccount(); if (!account) return;
    const data = await accountRequest<{ browsers: BrowserRow[] }>('/relay/account/browsers');
    if (!Array.isArray(data.browsers)) throw new Error(translate('auth.accountCertificateInvalid'));
    for (const row of data.browsers) if (!await verifyBrowserCertificate(row.certificate, account)) throw new Error(translate('auth.accountCertificateInvalid'));
    const revocations = await accountRevocations(true);
    setBrowsers(data.browsers.filter(b => !revocations.some(r => r.kind === 'browser' && r.subjectId === b.certificate.browserId)));
    setThisId((await currentBrowser())?.browserId); setCode(await safetyCode(account.accountKeyId));
  }
  useEffect(() => { void load().catch(e => setError(e instanceof Error ? e.message : translate('auth.accountRequestFailed'))); }, []);
  const [verification, setVerification] = useState<SecurityStatus>();
  const retry = useRef<(() => Promise<void>) | null>(null);
  async function run(work: () => Promise<void>, verified = false) {
    setBusy(true); setError(undefined); setMessage(undefined);
    try { await work(); await load(); } catch (e) {
      setError(e instanceof Error ? e.message : translate('auth.accountRequestFailed'));
      if (!verified && (e as { code?: string }).code === 'reauthentication_required') {
        try { const status = await securityRequest<SecurityStatus>('');
          if (status.authenticatorEnabled || status.passkeys.length) { retry.current = work; setVerification(status); }
        } catch { /* The original action error remains visible. */ }
      }
    }
    finally { setBusy(false); setPassword(''); }
  }
  const date = (n: number) => new Date(n).toLocaleString(getLocale());
  return <section className="space-y-4 border-t border-[var(--theme-border)] py-6">
    <h2 className="font-semibold">{translate('auth.accountDeviceSecurity')}</h2>
    <p className="text-sm text-[var(--theme-fg-muted)]">{translate('auth.accountPasswordHelp')}</p>
    <p>{translate('auth.accountSafetyCode')} <code className="font-mono">{code}</code></p>
    {error && <p role="alert">{error}</p>}{message && <p role="status">{message}</p>}
    <h3>{translate('auth.accountBrowsers')}</h3>
    <ul className="space-y-3">{browsers.map(b => <li className="owner-trust-row" key={b.certificate.browserId}>
      <span className="min-w-0 break-words" title={b.certificate.browserId}>{b.label || `${b.certificate.browserId.slice(0, 8)}…`}
        {b.certificate.browserId === thisId && <span className="owner-trust-browser-badge ml-2">{translate('auth.ownerCurrent')}</span>}
        <span className="block text-xs text-[var(--theme-fg-muted)]">{translate('auth.accountCreated', { value1: date(b.createdAt) })}</span>
        <span className="block text-xs text-[var(--theme-fg-muted)]">{translate('auth.accountLastSeen', { value1: date(b.lastSeenAt) })}</span>
      </span>
      <button className="relay-button-secondary min-h-11 px-3" disabled={busy} onClick={() => {
        if (window.confirm(translate('auth.accountRemoveBrowserConfirm', { value1: b.label || b.certificate.browserId.slice(0, 8) })))
          void run(async () => { await revokeAccountSubject('browser', b.certificate.browserId); });
      }}>{translate('auth.accountRemoveBrowser')}</button>
    </li>)}</ul>
    <div className="flex flex-wrap gap-2">
      <button className="relay-button-secondary min-h-11 px-3" disabled={busy} onClick={() => setAction('password')}>{translate('auth.changePassword')}</button>
      <button className="relay-button-secondary min-h-11 px-3" disabled={busy} onClick={() => setAction('recovery')}>{translate('auth.accountRegenerateRecovery')}</button>
      {canStartOver && <button className="relay-button-secondary min-h-11 px-3" disabled={busy} onClick={() => setAction('reset')}>{translate('auth.accountStartOver')}</button>}
    </div>
    {verification && <SecurityVerification status={verification} onCancel={() => { retry.current = null; setVerification(undefined); }} onVerified={async () => { const work = retry.current; retry.current = null; setVerification(undefined); if (work) await run(work, true); }} />}
    {action && !verification && <FormDialog title={translate(action === 'password' ? 'auth.changePassword' : action === 'reset' ? 'auth.accountStartOver' : 'auth.accountRegenerateRecovery')} busy={busy} onClose={() => { setAction(undefined); setPassword(''); }}>
      <p>{translate(action === 'reset' ? 'auth.accountResetWarning' : 'auth.accountPasswordHelp')}</p>
      <form className="space-y-4" onSubmit={e => { e.preventDefault(); void run(async () => {
        if (action === 'password') { await changeAccountPassword(password, await isOAuthAccount()); setMessage(translate('auth.passwordChanged')); }
        else if (action === 'reset') { await startAccountOver(); location.assign('/relay-portal'); }
        else setRecovery(await regenerateRecoveryCode());
        setAction(undefined);
      }); }}>
        {action === 'password' && <label className="block">{translate('auth.newPassword')}<input type="password" autoComplete="new-password" minLength={12} className="relay-input mt-2 w-full" value={password} required onChange={e => setPassword(e.target.value)} disabled={busy} /></label>}
        {error && <p role="alert">{error}</p>}
        <button className="relay-button-primary min-h-11 px-4" disabled={busy}>{translate('auth.continue')}</button>
      </form>
    </FormDialog>}
    {recovery && <FormDialog title={translate('auth.accountRecoveryCode')} onClose={() => {}}>
      <RecoveryConfirmation code={recovery} onConfirm={() => setRecovery(undefined)} />
    </FormDialog>}
  </section>;
}
