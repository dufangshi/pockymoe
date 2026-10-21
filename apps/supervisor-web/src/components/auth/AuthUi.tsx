import { appUrl } from '../../lib/webBase';
import { ErrorMark } from '@pockymoe/thread-ui';
import { LanguageSwitcher, translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { Check, Copy, Download, Eye, EyeOff } from 'lucide-react';
import { useId, useState, type ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { relayUrl } from '../../lib/relayOrigin';

// Building blocks shared by every sign-in, setup and unlock screen. The page
// keeps `data-page="relay-portal"` so theme packs can restyle it.

export function AuthShell({ children }: { children: ReactNode }) {
  useI18n();
  return (
    <main className="auth-shell flex min-h-[100dvh] flex-col bg-[var(--app-bg)] text-[var(--app-fg)]" data-page="relay-portal">
      <header className="auth-topbar mx-auto flex w-full max-w-5xl items-center justify-between gap-3 px-4 py-4 sm:px-6">
        <Link className="auth-brand flex min-h-11 items-center gap-2.5 text-[15px] font-semibold text-[var(--theme-fg)]" to="/">
          <img alt="" className="h-8 w-8 rounded-lg" src={appUrl('icon-192.png?v=pockymoe')} />
          <span>Pockymoe</span>
        </Link>
        <div className="flex items-center gap-1">
          <LanguageSwitcher className="auth-language" />
          <Link className="auth-topbar-link inline-flex min-h-11 items-center rounded-lg px-3 text-sm text-[var(--theme-fg-muted)] hover:text-[var(--theme-fg)]" to="/relay-guide">
            {translate('auth.guide')}
          </Link>
          <Link className="auth-topbar-link inline-flex min-h-11 items-center rounded-lg px-3 text-sm text-[var(--theme-fg-muted)] hover:text-[var(--theme-fg)]" to="/help/account">
            {translate('common.help')}
          </Link>
        </div>
      </header>
      <div className="flex flex-1 justify-center px-4 pb-16 pt-4 sm:items-center sm:pb-24 sm:pt-0">{children}</div>
    </main>
  );
}

export function AuthCard({ title, subtitle, children, footer, icon }: {
  title: string;
  subtitle?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  icon?: ReactNode;
}) {
  return (
    <section className="auth-card h-fit w-full max-w-[400px] rounded-2xl border border-[var(--theme-border)] bg-[var(--theme-panel)] p-6 shadow-[var(--theme-shadow)] sm:p-8">
      {icon ? <div className="auth-card-icon mb-4 text-[var(--theme-accent-strong)]">{icon}</div> : null}
      <h1 className="text-[22px] font-semibold leading-tight tracking-tight text-[var(--theme-fg)]">{title}</h1>
      {subtitle ? <p className="auth-subtitle mt-2 text-sm leading-6 text-[var(--theme-fg-muted)]">{subtitle}</p> : null}
      <div className="auth-card-body mt-6 space-y-4">{children}</div>
      {footer ? <div className="auth-footer mt-6 border-t border-[var(--theme-border)] pt-4 text-center text-sm text-[var(--theme-fg-muted)]">{footer}</div> : null}
    </section>
  );
}

export function AuthField({ label, value, onChange, type = 'text', autoComplete, hint, aside, autoFocus, disabled, minLength, required = true, name, inputMode, spellCheck }: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  type?: 'text' | 'email' | 'password';
  autoComplete?: string;
  hint?: string;
  aside?: ReactNode;
  autoFocus?: boolean;
  disabled?: boolean;
  minLength?: number;
  required?: boolean;
  name?: string;
  inputMode?: 'text' | 'email';
  spellCheck?: boolean;
}) {
  const id = useId(), hintId = useId();
  const [visible, setVisible] = useState(false);
  const secret = type === 'password';
  return (
    <div className="auth-field">
      <div className="mb-1.5 flex items-baseline justify-between gap-3 text-sm">
        <label className="text-sm font-medium text-[var(--theme-fg)]" htmlFor={id}>{label}</label>
        {aside}
      </div>
      <div className="relative">
        <input
          aria-describedby={hint ? hintId : undefined}
          autoComplete={autoComplete}
          autoFocus={autoFocus}
          className={`relay-input min-h-11 w-full ${secret ? 'pr-11' : ''}`}
          disabled={disabled}
          id={id}
          inputMode={inputMode}
          minLength={minLength}
          name={name}
          onChange={event => onChange(event.target.value)}
          required={required}
          spellCheck={spellCheck}
          type={secret && visible ? 'text' : type}
          value={value}
        />
        {secret ? (
          <button
            aria-label={translate(visible ? 'auth.hide' : 'auth.show', { value1: label.toLowerCase() })}
            className="absolute inset-y-0 right-0 inline-flex w-11 items-center justify-center text-[var(--theme-fg-muted)] hover:text-[var(--theme-fg)]"
            disabled={disabled}
            onClick={() => setVisible(v => !v)}
            type="button"
          >
            {visible ? <EyeOff aria-hidden="true" className="h-4 w-4" /> : <Eye aria-hidden="true" className="h-4 w-4" />}
          </button>
        ) : null}
      </div>
      {hint ? <p className="mt-1.5 text-xs text-[var(--theme-fg-muted)]" id={hintId}>{hint}</p> : null}
    </div>
  );
}

export function AuthNotice({ tone = 'danger', children }: { tone?: 'danger' | 'info'; children: ReactNode }) {
  // A failure reason is a quiet mark that opens the reason on a tap, not a red
  // paragraph (ui-principles §11). Prefer `AuthSubmit`'s `error`, which puts
  // the mark right after the button that failed.
  if (tone === 'danger' && typeof children === 'string') {
    return <div className="auth-notice-mark flex items-center"><ErrorMark message={children} /></div>;
  }
  return (
    <div
      aria-live={tone === 'danger' ? 'assertive' : 'polite'}
      className={`auth-notice rounded-xl px-3.5 py-2.5 text-sm leading-6 ${tone === 'danger'
        ? 'bg-[var(--status-danger-bg)] text-[var(--status-danger-fg)]'
        : 'bg-[var(--theme-accent-soft)] text-[var(--theme-fg)]'}`}
      role={tone === 'danger' ? 'alert' : 'status'}
    >
      {children}
    </div>
  );
}

export function AuthSubmit({ busy, children, disabled, error, onRetry }: {
  busy?: boolean;
  children: ReactNode;
  disabled?: boolean;
  /** Why the last submit failed: a mark right after the button. */
  error?: string | null | undefined;
  onRetry?: (() => void) | undefined;
}) {
  return (
    <div className="flex items-center gap-2">
      <button aria-busy={busy || undefined} className="relay-button-primary auth-submit h-11 w-full min-w-0" disabled={busy || disabled} type="submit">
        {busy ? translate('auth.ui.pleaseWait') : children}
      </button>
      {error ? <ErrorMark message={error} onRetry={onRetry} /> : null}
    </div>
  );
}

export function AuthLink({ onClick, children, disabled }: { onClick: () => void; children: ReactNode; disabled?: boolean | undefined }) {
  return (
    <button className="auth-link text-sm font-medium text-[var(--theme-accent-strong)] hover:underline disabled:opacity-60" disabled={disabled} onClick={onClick} type="button">
      {children}
    </button>
  );
}

export function AuthDivider() {
  return (
    <div className="auth-divider flex items-center gap-3 text-xs text-[var(--theme-fg-muted)]">
      <span className="h-px flex-1 bg-[var(--theme-border)]" />
      <span>{translate('auth.ui.or')}</span>
      <span className="h-px flex-1 bg-[var(--theme-border)]" />
    </div>
  );
}

function GoogleMark() {
  return (
    <svg aria-hidden="true" className="h-[18px] w-[18px]" viewBox="0 0 24 24">
      <path d="M21.6 12.23c0-.68-.06-1.36-.18-2.02H12v3.83h5.4a4.6 4.6 0 0 1-2 3.02v2.5h3.24c1.9-1.75 2.96-4.33 2.96-7.33z" fill="#4285F4" />
      <path d="M12 22c2.7 0 4.96-.9 6.62-2.43l-3.24-2.5c-.9.6-2.04.96-3.38.96-2.6 0-4.8-1.75-5.6-4.11H3.06v2.58A10 10 0 0 0 12 22z" fill="#34A853" />
      <path d="M6.4 13.92a6 6 0 0 1 0-3.84V7.5H3.06a10 10 0 0 0 0 9z" fill="#FBBC05" />
      <path d="M12 5.98c1.47 0 2.79.5 3.83 1.5l2.87-2.87A9.6 9.6 0 0 0 12 2a10 10 0 0 0-8.94 5.5l3.34 2.58C7.2 7.73 9.4 5.98 12 5.98z" fill="#EA4335" />
    </svg>
  );
}

function GitHubMark() {
  return (
    <svg aria-hidden="true" className="h-[18px] w-[18px]" fill="currentColor" viewBox="0 0 24 24">
      <path d="M12 2a10 10 0 0 0-3.16 19.49c.5.09.68-.22.68-.48v-1.7c-2.78.6-3.37-1.34-3.37-1.34-.46-1.16-1.11-1.47-1.11-1.47-.91-.62.07-.6.07-.6 1 .07 1.53 1.03 1.53 1.03.9 1.52 2.34 1.08 2.91.83.09-.65.35-1.09.63-1.34-2.22-.25-4.55-1.11-4.55-4.94 0-1.09.39-1.98 1.03-2.68-.1-.25-.45-1.27.1-2.65 0 0 .84-.27 2.75 1.02a9.5 9.5 0 0 1 5 0c1.91-1.29 2.75-1.02 2.75-1.02.55 1.38.2 2.4.1 2.65.64.7 1.03 1.59 1.03 2.68 0 3.84-2.34 4.69-4.57 4.93.36.31.68.92.68 1.85v2.74c0 .27.18.58.69.48A10 10 0 0 0 12 2z" />
    </svg>
  );
}

export function OAuthButtons({ google, github }: { google?: boolean; github?: boolean }) {
  if (!google && !github) return null;
  // Providers return to /relay-portal, which restores the remembered destination.
  const href = (provider: string) => relayUrl(`relay/auth/oauth/${provider}/start`);
  return (
    <div className="auth-oauth grid gap-2.5">
      {google ? (
        <a className="relay-button-secondary inline-flex h-11 items-center justify-center gap-2.5 font-medium" href={href('google')}>
          <GoogleMark />{translate('auth.continueWithGoogle')}
        </a>
      ) : null}
      {github ? (
        <a className="relay-button-secondary inline-flex h-11 items-center justify-center gap-2.5 font-medium" href={href('github')}>
          <GitHubMark />{translate('auth.continueWithGitHub')}
        </a>
      ) : null}
    </div>
  );
}

/** A freshly stored recovery code with copy, download and a short confirmation. */
export function RecoveryCodeBody({ code, onDone, busy = false, doneLabel }: { code: string; onDone: () => void; busy?: boolean; doneLabel?: string }) {
  useI18n();
  const [typed, setTyped] = useState(''), [copied, setCopied] = useState(false);
  const confirmed = typed.trim().toUpperCase() === code.replace(/-/g, '').slice(-4);
  const download = () => {
    const url = URL.createObjectURL(new Blob([`Pockymoe recovery code\n\n${code}\n`], { type: 'text/plain' }));
    const link = Object.assign(document.createElement('a'), { href: url, download: 'pockymoe-recovery-code.txt' });
    link.click(); URL.revokeObjectURL(url);
  };
  return (
    <div className="space-y-4">
      <div aria-label={translate('auth.ui.recoveryTitle')} className="auth-recovery-code grid grid-cols-4 gap-2 rounded-xl bg-[var(--theme-muted)] p-3 font-mono text-[15px] font-semibold tracking-wider text-[var(--theme-fg)]" role="group">
        {code.split('-').map((group, index) => <span className="text-center" key={index}>{group}</span>)}
      </div>
      <div className="grid grid-cols-2 gap-2">
        <button className="relay-button-secondary inline-flex h-10 items-center justify-center gap-2" type="button"
          onClick={() => void navigator.clipboard?.writeText(code).then(() => setCopied(true)).catch(() => undefined)}>
          {copied ? <Check aria-hidden="true" className="h-4 w-4" /> : <Copy aria-hidden="true" className="h-4 w-4" />}
          {translate(copied ? 'auth.ui.copied' : 'auth.ui.copy')}
        </button>
        <button className="relay-button-secondary inline-flex h-10 items-center justify-center gap-2" onClick={download} type="button">
          <Download aria-hidden="true" className="h-4 w-4" />{translate('auth.ui.download')}
        </button>
      </div>
      <form className="space-y-4" onSubmit={event => { event.preventDefault(); if (confirmed) onDone(); }}>
        <AuthField autoComplete="off" label={translate('auth.ui.recoveryConfirm')} onChange={setTyped} spellCheck={false} value={typed} />
        <AuthSubmit busy={busy} disabled={!confirmed}>{doneLabel ?? translate('auth.ui.recoverySaved')}</AuthSubmit>
      </form>
    </div>
  );
}

/** Shows a freshly created recovery code once, after it is stored in the vault. */
export function RecoveryCodeStep({ code, onDone, busy = false }: { code: string; onDone: () => void; busy?: boolean }) {
  useI18n();
  return (
    <AuthCard subtitle={translate('auth.ui.recoveryBody')} title={translate('auth.ui.recoveryTitle')}>
      <RecoveryCodeBody busy={busy} code={code} onDone={onDone} />
    </AuthCard>
  );
}

/** "Signed in as … · Sign out", shown under every post-login screen. */
export function SignedInFooter({ name, onSignOut, busy }: { name?: string | null | undefined; onSignOut: () => void; busy?: boolean | undefined }) {
  return (
    <span className="inline-flex flex-wrap items-center justify-center gap-x-2 gap-y-1">
      {name ? <span>{translate('auth.ui.signedInAs', { value1: name })}</span> : null}
      <AuthLink disabled={busy} onClick={onSignOut}>{translate('auth.ui.signOut')}</AuthLink>
    </span>
  );
}

export function AuthLoading() {
  return (
    <AuthShell>
      <p aria-live="polite" className="py-24 text-sm text-[var(--theme-fg-muted)]" role="status">{translate('auth.ui.loading')}</p>
    </AuthShell>
  );
}
