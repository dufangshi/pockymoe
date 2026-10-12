import { getLocale } from '@pockymoe/thread-ui/i18n';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { FormDialog } from './FormDialog';
import { useDialogLifecycle } from './useDialogLifecycle';
import { request } from '../lib/api';
import { useEffect, useRef, useState, type FormEvent } from 'react';
import {
  Check,
  Copy,
  Download,
  Fingerprint,
  KeyRound,
  ShieldCheck,
  Smartphone,
  Trash2,
  X,
} from 'lucide-react';
import { createPortal } from 'react-dom';
import {
  authenticatePasskey,
  fetchChallenge,
  registerPasskey,
  securityError,
  securityRequest,
  verifyLoginCode,
  type Enrollment,
  type LoginChallenge,
  type SecurityStatus,
  type SecurityRealm,
} from '../lib/relaySecurity';

const input = 'relay-input min-h-11 w-full';
const secondary =
  'relay-button-secondary inline-flex min-h-10 items-center justify-center gap-2 px-3 text-sm disabled:opacity-50';
function ErrorText({ error }: { error: string | null }) {
  const { locale: i18nLocale } = useI18n();
  return error ? (
    <p role="alert" className="text-sm text-[var(--status-danger-fg)]">
      {error}
    </p>
  ) : null;
}

export function LoginVerification({
  challenge,
  onSuccess,
  onBack,
}: {
  challenge: LoginChallenge;
  onSuccess: () => Promise<void>;
  onBack: () => void;
}) {
  const { locale: i18nLocale } = useI18n();
  const [code, setCode] = useState(''),
    [remember, setRemember] = useState(true),
    [recovery, setRecovery] = useState(false);
  const [busy, setBusy] = useState(false),
    [error, setError] = useState<string | null>(null);
  async function verify(passkey = false) {
    setBusy(true);
    setError(null);
    try {
      if (passkey) await authenticatePasskey('login', remember);
      else await verifyLoginCode(code, remember && !recovery);
      await onSuccess();
    } catch (e) {
      setError(securityError(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="w-full max-w-md space-y-5 rounded-xl border border-[var(--theme-border)] bg-[var(--theme-panel)] p-6 shadow-[var(--theme-shadow)]">
      <ShieldCheck
        className="h-8 w-8 text-[var(--theme-accent-strong)]"
        aria-hidden="true"
      />
      <div>
        <h1 className="text-xl font-semibold">{translate("auth.verifyItSYou")}</h1>
        <p className="mt-2 text-sm text-[var(--theme-fg-muted)]">
          {translate("auth.oneMoreStepForThisBrowser")}</p>
      </div>
      {challenge.passkey && (
        <button
          className={`${secondary} w-full`}
          disabled={busy}
          onClick={() => void verify(true)}
        >
          <Fingerprint size={18} /> {translate("auth.useAPasskey")}</button>
      )}
      {(challenge.authenticator || recovery) && (
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            void verify();
          }}
        >
          <label className="block text-sm">
            {recovery ? translate("auth.recoveryCode") : translate("auth.authenticatorCode")}
            <input
              autoFocus
              autoComplete="one-time-code"
              inputMode={recovery ? 'text' : 'numeric'}
              className={`${input} mt-2 font-mono tracking-widest`}
              value={code}
              onChange={(e) => setCode(e.target.value)}
              maxLength={recovery ? 24 : 6}
              disabled={busy}
              required
            />
          </label>
          {!recovery && (
            <label className="flex items-center gap-2 text-sm text-[var(--theme-fg-muted)]">
              <input
                type="checkbox"
                checked={remember}
                onChange={(e) => setRemember(e.target.checked)}
              />{' '}
              {translate("auth.trustThisBrowserFor30Days")}</label>
          )}
          <button
            className="relay-button-primary min-h-11 w-full"
            disabled={busy || !code.trim()}
          >
            {busy ? translate("auth.verifying") : translate("auth.verify")}
          </button>
        </form>
      )}
      {!challenge.authenticator && !recovery && (
        <label className="flex items-center gap-2 text-sm text-[var(--theme-fg-muted)]">
          <input
            type="checkbox"
            checked={remember}
            onChange={(e) => setRemember(e.target.checked)}
          />{' '}
          {translate("auth.trustThisBrowserFor30Days")}</label>
      )}
      <ErrorText error={error} />
      <div className="flex flex-wrap justify-between gap-3 text-sm">
        <button
          type="button"
          className="text-[var(--theme-fg-muted)] hover:text-[var(--theme-fg)]"
          onClick={() => {
            setRecovery(!recovery);
            setCode('');
            setError(null);
          }}
        >
          {recovery ? translate("auth.useAnotherMethod") : translate("auth.useARecoveryCode")}
        </button>
        <button
          type="button"
          className="text-[var(--theme-fg-muted)]"
          disabled={busy}
          onClick={() =>
            void request(
              '/relay/auth/challenge',
              { method: 'DELETE' },
              { auth: 'none' },
            )
              .then(onBack)
              .catch((e) => setError(securityError(e)))
          }
        >
          {translate("auth.backToSignIn")}</button>
      </div>
    </section>
  );
}

export function usePendingLogin() {
  const { locale: i18nLocale } = useI18n();
  const [challenge, setChallenge] = useState<LoginChallenge | null>(null);
  useEffect(() => {
    let active = true;
    void fetchChallenge()
      .then((value) => {
        if (active && value.challengeRequired === true) setChallenge(value);
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, []);
  return [challenge, setChallenge] as const;
}

export function SecurityVerification({
  status,
  onVerified,
  onCancel,
  realm = 'default',
}: {
  realm?: SecurityRealm;
  status: Pick<SecurityStatus, 'authenticatorEnabled' | 'passkeys'>;
  onVerified: (verificationToken?: string) => Promise<void>;
  onCancel: () => void;
}) {
  const { locale: i18nLocale } = useI18n();
  const [value, setValue] = useState(''),
    [busy, setBusy] = useState(false),
    [error, setError] = useState<string | null>(null);
  const factor = status.authenticatorEnabled || status.passkeys.length > 0;
  async function verify(passkey = false) {
    setBusy(true);
    setError(null);
    try {
      const result = passkey
        ? await authenticatePasskey('reauth', false, realm)
        : await securityRequest<{ verificationToken?: string }>(
            '/reauth',
            'POST',
            factor ? { code: value } : { password: value },
            realm,
          );
      await onVerified(result.verificationToken);
    } catch (e) {
      setError(securityError(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <FormDialog title={translate("auth.verifyYourIdentity")} onClose={onCancel} busy={busy}>
      <p className="text-sm text-[var(--theme-fg-muted)]">
        {translate("auth.confirmThisSecurityChangeWithYourAuthenticator")}</p>
      {status.passkeys.length > 0 && (
        <button
          className={`${secondary} w-full`}
          disabled={busy}
          onClick={() => void verify(true)}
        >
          <Fingerprint size={16} /> {translate("auth.useAPasskey")}</button>
      )}
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          void verify();
        }}
      >
        <label className="block text-sm">
          {factor ? translate("auth.authenticatorOrRecoveryCode") : translate("auth.password")}
          <input
            autoFocus
            className={`${input} mt-2`}
            type={factor ? 'text' : 'password'}
            autoComplete={factor ? 'one-time-code' : 'current-password'}
            value={value}
            onChange={(e) => setValue(e.target.value)}
            disabled={busy}
            required
          />
        </label>
        <ErrorText error={error} />
        <div className="flex justify-end gap-2">
          <button
            type="button"
            className={secondary}
            disabled={busy}
            onClick={onCancel}
          >
            {translate("auth.cancel")}</button>
          <button
            className="relay-button-primary min-h-10 px-4"
            disabled={busy || !value}
          >
            {busy ? translate("auth.verifying") : translate("auth.verify")}
          </button>
        </div>
      </form>
    </FormDialog>
  );
}

function RecoveryCodes({
  codes,
  onClose,
}: {
  codes: string[];
  onClose: () => void;
}) {
  const { locale: i18nLocale } = useI18n();
  const [copied, setCopied] = useState(false),
    [saved, setSaved] = useState(false),
    [error, setError] = useState<string | null>(null);
  const text = `Pockymoe recovery codes\nKeep these somewhere private. Each code works once.\n\n${codes.join('\n')}\n`;
  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setSaved(true);
    } catch {
      setError(translate("auth.copyWasUnavailableDownloadTheCodesInstead"));
    }
  }
  function download() {
    const url = URL.createObjectURL(new Blob([text], { type: 'text/plain' }));
    const a = document.createElement('a');
    a.href = url;
    a.download = 'pockymoe-recovery-codes.txt';
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    setSaved(true);
  }
  return (
    <div
      className="space-y-4 rounded-lg border border-[var(--theme-accent-strong)] p-4"
      role="region"
      aria-label={translate("auth.saveRecoveryCodes")}
    >
      <h3 className="font-medium">{translate("auth.saveYourRecoveryCodes")}</h3>
      <p className="text-sm text-[var(--theme-fg-muted)]">
        {translate("auth.useTheseIfYouLoseYourAuthenticator")}</p>
      <div className="grid gap-2 rounded-md bg-[var(--theme-bg)] p-3 font-mono text-sm sm:grid-cols-2">
        {codes.map((code) => (
          <span key={code}>{code}</span>
        ))}
      </div>
      <ErrorText error={error} />
      <div className="flex flex-wrap gap-2">
        <button className={secondary} onClick={() => void copy()}>
          {copied ? <Check size={15} /> : <Copy size={15} />} {translate("auth.copy")}</button>
        <button className={secondary} onClick={download}>
          <Download size={15} /> {translate("auth.download")}</button>
        <button
          className="relay-button-primary px-4"
          disabled={!saved}
          onClick={onClose}
        >
          {translate("auth.done")}</button>
      </div>
    </div>
  );
}

export function RelaySecurityPanel({
  realm = 'default',
}: {
  realm?: SecurityRealm;
}) {
  const { locale: i18nLocale } = useI18n();
  const call = <T,>(path: string, method = 'GET', body?: unknown) =>
    securityRequest<T>(path, method, body, realm);
  const [status, setStatus] = useState<SecurityStatus | null>(null),
    [error, setError] = useState<string | null>(null),
    [busy, setBusy] = useState(false);
  const [enrollment, setEnrollment] = useState<Enrollment | null>(null),
    [code, setCode] = useState(''),
    [codes, setCodes] = useState<string[] | null>(null);
  const [keyName, setKeyName] = useState(''),
    [addingKey, setAddingKey] = useState(false),
    [editing, setEditing] = useState<string | null>(null);
  const [verifyAction, setVerifyAction] = useState<
    (() => Promise<void>) | null
  >(null);
  async function load() {
    const value = await call<SecurityStatus>('');
    setStatus(value);
  }
  useEffect(() => {
    void load().catch((e) => setError(securityError(e)));
  }, []);
  async function run(action: () => Promise<void>, verified = false) {
    if (!verified && status && !status.recentlyVerified) {
      setVerifyAction(() => action);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await action();
      await load();
    } catch (e) {
      if (
        e instanceof Error &&
        'payload' in e &&
        (e as { payload?: { code?: string } }).payload?.code ===
          'reauthentication_required'
      )
        setVerifyAction(() => action);
      else if (
        e instanceof Error &&
        'payload' in e &&
        (e as { payload?: { code?: string } }).payload?.code === 'unauthorized'
      )
        location.assign('/relay');
      else setError(securityError(e));
    } finally {
      setBusy(false);
    }
  }
  async function confirm(event: FormEvent) {
    event.preventDefault();
    await run(async () => {
      const result = await call<{ recoveryCodes: string[] }>(
        '/authenticator/confirm',
        'POST',
        { code },
      );
      setEnrollment(null);
      setCode('');
      setCodes(result.recoveryCodes);
    });
  }
  const date = (value: number) => new Date(value).toLocaleDateString(getLocale());
  return (
    <section className="grid gap-5 py-6 sm:grid-cols-[11rem_minmax(0,1fr)] sm:gap-8">
      <header>
        <h2 className="flex items-center gap-2 text-base font-semibold">
          <ShieldCheck size={18} /> {translate("auth.security")}</h2>
        <p className="mt-1 text-sm text-[var(--theme-fg-muted)]">
          {translate("auth.protectYourAccountAndDevices")}</p>
      </header>
      <div className="min-w-0 space-y-5">
        <ErrorText error={error} />
        {!status && !error && (
          <p role="status" className="text-sm text-[var(--theme-fg-muted)]">
            {translate("auth.loadingSecuritySettings")}</p>
        )}
        {status && (
          <>
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <h3 className="flex items-center gap-2 font-medium">
                  <Smartphone size={17} /> {translate("auth.authenticatorApp")}{' '}
                  {status.authenticatorEnabled && (
                    <Check
                      size={15}
                      className="text-[var(--status-success-fg)]"
                    />
                  )}
                </h3>
                <p className="mt-1 text-sm text-[var(--theme-fg-muted)]">
                  {translate("auth.googleAuthenticatorAndCompatibleApps")}</p>
              </div>
              <button
                className={secondary}
                disabled={busy}
                onClick={() =>
                  void run(async () => {
                    if (status.authenticatorEnabled)
                      await call('/authenticator', 'DELETE');
                    else
                      setEnrollment(
                        await call<Enrollment>('/authenticator/enroll', 'POST'),
                      );
                  })
                }
              >
                {status.authenticatorEnabled ? translate("auth.disable") : translate("auth.setUp")}
              </button>
            </div>
            {enrollment && (
              <form
                onSubmit={(e) => void confirm(e)}
                className="space-y-4 rounded-lg border border-[var(--theme-border)] p-4"
              >
                <p className="text-sm">
                  {translate("auth.scanThisCodeInYourAuthenticatorThen")}</p>
                <img
                  alt={translate("auth.authenticatorSetupQRCode")}
                  className="h-44 w-44 rounded-md bg-white"
                  src={`data:image/svg+xml;base64,${btoa(enrollment.qrSvg)}`}
                />
                <details className="text-sm">
                  <summary className="cursor-pointer text-[var(--theme-fg-muted)]">
                    {translate("auth.canTScanTheCode")}</summary>
                  <code className="mt-2 block break-all select-all rounded bg-[var(--theme-bg)] p-2">
                    {enrollment.secret}
                  </code>
                </details>
                <input
                  aria-label={translate("auth.setupVerificationCode")}
                  className={`${input} max-w-xs font-mono tracking-widest`}
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  value={code}
                  onChange={(e) => setCode(e.target.value)}
                  maxLength={6}
                  required
                />
                <div className="flex gap-2">
                  <button
                    className="relay-button-primary min-h-10 px-4"
                    disabled={busy || code.length !== 6}
                  >
                    {translate("auth.enableAuthenticator")}</button>
                  <button
                    className={secondary}
                    type="button"
                    onClick={() => {
                      setEnrollment(null);
                      setCode('');
                    }}
                  >
                    {translate("auth.cancel")}</button>
                </div>
              </form>
            )}
            {codes && (
              <RecoveryCodes codes={codes} onClose={() => setCodes(null)} />
            )}
            <div className="border-t border-[var(--theme-border)] pt-4">
              <div className="flex items-center justify-between gap-3">
                <h3 className="flex items-center gap-2 font-medium">
                  <Fingerprint size={17} /> {translate("auth.passkeys")}</h3>
                {status.passkeyAvailable && (
                  <button
                    className={secondary}
                    disabled={busy}
                    onClick={() => {
                      setAddingKey(true);
                      setKeyName('');
                    }}
                  >
                    {translate("auth.addPasskey")}</button>
                )}
              </div>
              {!status.passkeyAvailable && (
                <p className="mt-2 text-sm text-[var(--theme-fg-muted)]">
                  {translate("auth.passkeySetupIsUnavailableOnThisRelay")}</p>
              )}
              {addingKey && (
                <form
                  className="mt-3 flex flex-wrap gap-2"
                  onSubmit={(e) => {
                    e.preventDefault();
                    void run(async () => {
                      const result = await registerPasskey(keyName, realm);
                      setAddingKey(false);
                      setKeyName('');
                      if (result.recoveryCodes) setCodes(result.recoveryCodes);
                    });
                  }}
                >
                  <input
                    aria-label={translate("auth.passkeyName")}
                    placeholder={translate("auth.eGMyPhone")}
                    className={`${input} min-w-0 flex-1`}
                    maxLength={80}
                    value={keyName}
                    onChange={(e) => setKeyName(e.target.value)}
                    required
                  />
                  <button
                    className={secondary}
                    disabled={busy || !keyName.trim()}
                  >
                    {translate("auth.continue")}</button>
                  <button
                    type="button"
                    className={secondary}
                    onClick={() => setAddingKey(false)}
                    aria-label={translate("auth.cancelPasskeySetup")}
                  >
                    <X size={15} />
                  </button>
                </form>
              )}
              <ul className="mt-2 divide-y divide-[var(--theme-border)]">
                {status.passkeys.map((key) => (
                  <li
                    key={key.id}
                    className="flex flex-wrap items-center justify-between gap-2 py-3"
                  >
                    <div className="min-w-0">
                      <p className="break-words text-sm">{key.name}</p>
                      <p className="mt-1 text-xs text-[var(--theme-fg-muted)]">
                        {translate("auth.added")} {date(key.createdAt)}
                        {key.lastUsedAt
                          ? translate("auth.used", { value1: date(key.lastUsedAt) })
                          : ''}
                      </p>
                    </div>
                    <div className="flex gap-2">
                      {editing === key.id ? (
                        <form
                          className="flex gap-2"
                          onSubmit={(e) => {
                            e.preventDefault();
                            void run(async () => {
                              await call(
                                `/passkeys/${encodeURIComponent(key.id)}`,
                                'PATCH',
                                { name: keyName },
                              );
                              setEditing(null);
                            });
                          }}
                        >
                          <input
                            aria-label={translate("auth.renamePasskey")}
                            className={input}
                            value={keyName}
                            maxLength={80}
                            onChange={(e) => setKeyName(e.target.value)}
                          />
                          <button className={secondary}>{translate("auth.save")}</button>
                        </form>
                      ) : (
                        <button
                          className={secondary}
                          onClick={() => {
                            setEditing(key.id);
                            setKeyName(key.name);
                          }}
                        >
                          {translate("auth.rename")}</button>
                      )}
                      <button
                        className={secondary}
                        aria-label={translate("auth.remove", { value1: key.name })}
                        disabled={busy}
                        onClick={() =>
                          void run(async () => {
                            await call(
                              `/passkeys/${encodeURIComponent(key.id)}`,
                              'DELETE',
                            );
                          })
                        }
                      >
                        <Trash2 size={15} />
                      </button>
                    </div>
                  </li>
                ))}
              </ul>
            </div>
            {(status.authenticatorEnabled || status.passkeys.length > 0) && (
              <div className="flex flex-wrap items-center justify-between gap-3 border-t border-[var(--theme-border)] pt-4">
                <div>
                  <h3 className="flex items-center gap-2 font-medium">
                    <KeyRound size={17} /> {translate("auth.recoveryCodes")}</h3>
                  <p className="mt-1 text-sm text-[var(--theme-fg-muted)]">
                    {status.recoveryCodesRemaining} {translate("auth.unusedNewCodesReplaceThePreviousSet")}</p>
                </div>
                <button
                  className={secondary}
                  disabled={busy}
                  onClick={() =>
                    void run(async () => {
                      const result = await call<{ recoveryCodes: string[] }>(
                        '/recovery-codes',
                        'POST',
                      );
                      setCodes(result.recoveryCodes);
                    })
                  }
                >
                  {translate("auth.generateNewCodes")}</button>
              </div>
            )}
            <div className="border-t border-[var(--theme-border)] pt-4">
              <h3 className="font-medium">{translate("auth.trustedBrowsers")}</h3>
              <p className="mt-1 text-sm text-[var(--theme-fg-muted)]">
                {translate("auth.noRepeatedCodesOnTrustedBrowsersFor")}</p>
              {status.trustedBrowsers.length === 0 && (
                <p className="mt-2 text-sm text-[var(--theme-fg-muted)]">
                  {translate("auth.noTrustedBrowsersYet")}</p>
              )}
              <ul className="divide-y divide-[var(--theme-border)]">
                {status.trustedBrowsers.map((browser) => (
                  <li
                    key={browser.id}
                    className="flex items-center justify-between gap-3 py-3"
                  >
                    <div>
                      <p className="text-sm">{browser.name}</p>
                      <p className="mt-1 text-xs text-[var(--theme-fg-muted)]">
                        {translate("auth.expires")} {date(browser.expiresAt)}
                      </p>
                    </div>
                    <button
                      className={secondary}
                      disabled={busy}
                      onClick={() =>
                        void run(async () => {
                          await call(`/browsers/${browser.id}`, 'DELETE');
                        })
                      }
                    >
                      {translate("auth.revoke")}</button>
                  </li>
                ))}
              </ul>
            </div>
            <div className="border-t border-[var(--theme-border)] pt-4">
              <h3 className="font-medium">{translate("auth.activeSessions")}</h3>
              <ul className="divide-y divide-[var(--theme-border)]">
                {status.sessions.map((session) => (
                  <li
                    key={session.id}
                    className="flex items-center justify-between gap-3 py-3"
                  >
                    <div>
                      <p className="text-sm">
                        {session.name || translate("auth.browser")}
                        {session.current && (
                          <span className="ml-2 text-xs text-[var(--theme-accent-strong)]">
                            {translate("auth.thisSession")}</span>
                        )}
                      </p>
                      <p className="mt-1 text-xs text-[var(--theme-fg-muted)]">
                        {translate("auth.signedIn")} {date(session.createdAt)}
                      </p>
                    </div>
                    {!session.current && (
                      <button
                        className={secondary}
                        disabled={busy}
                        onClick={() =>
                          void run(async () => {
                            await call(`/sessions/${session.id}`, 'DELETE');
                          })
                        }
                      >
                        {translate("auth.signOut")}</button>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          </>
        )}
        {verifyAction && status && (
          <SecurityVerification
            realm={realm}
            status={status}
            onCancel={() => setVerifyAction(null)}
            onVerified={async () => {
              const action = verifyAction;
              setVerifyAction(null);
              await run(action, true);
            }}
          />
        )}
      </div>
    </section>
  );
}
