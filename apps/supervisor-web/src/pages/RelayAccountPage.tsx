import { AccountSecurityPanel } from '../components/AccountSecurityPanel';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ErrorMark } from '@pockymoe/thread-ui';
import { ProductHeader } from '../components/ProductHeader';
import {
  RelaySecurityPanel,
} from '../components/RelaySecurity';
import { RefreshCw, Save } from 'lucide-react';
import { FormEvent, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';

import type { RelaySessionDto } from '@pockymoe/shared';
import {
  ApiError,
  enableRelayMode,
  fetchRelaySession,
  updateRelayAccount,
} from '../lib/api';

function errorMessage(caught: unknown, fallback: string) {
  return caught instanceof ApiError
    ? caught.payload.message
    : caught instanceof Error
      ? caught.message
      : fallback;
}

async function resizeAvatar(file: File): Promise<string> {
  if (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type) || file.size > 5 * 1024 * 1024) throw new Error('Invalid avatar');
  const url = URL.createObjectURL(file);
  try {
    const image = new Image(); image.src = url;
    await image.decode();
    const canvas = document.createElement('canvas'); canvas.width = canvas.height = 192;
    const context = canvas.getContext('2d');
    if (!context || !image.naturalWidth || !image.naturalHeight) throw new Error('Invalid image');
    const size = Math.min(image.naturalWidth, image.naturalHeight);
    context.drawImage(image, (image.naturalWidth - size) / 2, (image.naturalHeight - size) / 2, size, size, 0, 0, 192, 192);
    // Re-encoding strips metadata and bounds the persisted payload.
    const data = canvas.toDataURL('image/webp', 0.85);
    if (data.length > 131072) throw new Error('Avatar too large');
    return data;
  } finally { URL.revokeObjectURL(url); }
}

export function RelayAccountSettingsPanel({
  className = '',
}: {
  className?: string;
}) {
  const { locale: i18nLocale } = useI18n();
  const [session, setSession] = useState<RelaySessionDto | null>(null);
  const [username, setUsername] = useState('');
  const [avatar, setAvatar] = useState<string | null>(null);
  const [preparingAvatar, setPreparingAvatar] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [savingProfile, setSavingProfile] = useState(false);
  // Why the avatar or the save failed: a mark after that control.
  const [profileError, setProfileError] = useState<{ at: 'avatar' | 'save'; message: string } | null>(null);
  const [profileMessage, setProfileMessage] = useState<string | null>(null);

  async function load() {
    setLoading(true);
    setLoadError(null);
    try {
      enableRelayMode();
      const nextSession = await fetchRelaySession();
      setSession(nextSession);
      setUsername(nextSession.user?.username ?? '');
      setAvatar(nextSession.user?.avatarUrl ?? null);
    } catch (caught) {
      setSession(null);
      setLoadError(errorMessage(caught, translate("auth.accountLoadFailed")));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void load();
  }, []);

  async function saveProfile(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSavingProfile(true);
    setProfileError(null);
    setProfileMessage(null);
    try {
      const user = await updateRelayAccount({ username: username.trim(), avatarUrl: avatar });
      setSession((current) =>
        current?.authenticated ? { ...current, user } : current,
      );
      setUsername(user.username);
      setAvatar(user.avatarUrl ?? null);
      window.dispatchEvent(new Event('remote-codex-account-updated'));
      setProfileMessage(translate("auth.profileSaved"));
    } catch (caught) {
      setProfileError({ at: 'save', message: errorMessage(caught, translate("auth.profileSaveFailed")) });
    } finally {
      setSavingProfile(false);
    }
  }

  if (loading) {
    return (
      <div
        aria-live="polite"
        className={`space-y-4 ${className}`.trim()}
        role="status"
      >
        <span className="sr-only">{translate("auth.loadingAccount")}</span>
        <div
          className="h-4 w-28 animate-pulse rounded bg-[var(--theme-muted)]"
          aria-hidden="true"
        />
        <div
          className="h-11 w-full max-w-md animate-pulse rounded-lg bg-[var(--theme-muted)]"
          aria-hidden="true"
        />
        <div
          className="h-11 w-full max-w-md animate-pulse rounded-lg bg-[var(--theme-muted)]"
          aria-hidden="true"
        />
      </div>
    );
  }

  if (loadError) {
    // Nothing else can show: a short label, the reason behind a mark, and a visible Retry.
    return (
      <div className={className}>
        <p className="flex items-center gap-1.5 font-medium text-[var(--theme-fg)]">
          {translate("auth.accountDetailsCouldNotBeLoaded")}
          <ErrorMark message={loadError} />
        </p>
        <button
          className="relay-button-secondary mt-3 inline-flex h-11 items-center gap-2"
          onClick={() => void load()}
          type="button"
        >
          <RefreshCw aria-hidden="true" className="h-4 w-4" />
          {translate("auth.retry")}</button>
      </div>
    );
  }

  if (!session?.authenticated) {
    return (
      <div className={className}>
        <h2 className="text-base font-semibold text-[var(--theme-fg)]">
          {translate("auth.signInRequired")}</h2>
        <p className="mt-2 max-w-xl text-sm leading-6 text-[var(--theme-fg-muted)]">
          {translate("auth.yourRelaySessionHasEndedSignIn")}</p>
        <Link
          className="relay-button-primary mt-4 inline-flex h-11"
          to="/relay-portal?returnTo=%2Frelay-account"
        >
          {translate("auth.signIn")}</Link>
      </div>
    );
  }

  const profileDirty = username.trim() !== (session.user?.username ?? '') || avatar !== (session.user?.avatarUrl ?? null);

  return (
    <div
      className={`divide-y divide-[var(--theme-border)] border-y border-[var(--theme-border)] ${className}`.trim()}
    >
      <section className="grid gap-5 py-6 sm:grid-cols-[11rem_minmax(0,1fr)] sm:gap-8">
        <header>
          <h2 className="text-base font-semibold text-[var(--theme-fg)]">
            {translate("auth.profile")}</h2>
          <p className="mt-1 text-sm leading-5 text-[var(--theme-fg-muted)]">
            {translate("auth.yourRelayIdentity")}</p>
        </header>
        <div className="min-w-0 max-w-md">
          <div>
            <p className="text-sm text-[var(--theme-fg-soft)]">{translate("auth.email")}</p>
            <p className="mt-1 break-words text-sm font-medium text-[var(--theme-fg)]">
              {session.user?.email}
            </p>
          </div>
          <form className="mt-5 space-y-4" onSubmit={saveProfile}>
            <div className="flex items-center gap-4">
              <div className="flex h-16 w-16 shrink-0 items-center justify-center overflow-hidden rounded-full border border-[var(--theme-border)] bg-[var(--theme-surface-strong)] text-xl" aria-label={translate('auth.avatar')}>
                {avatar ? <img src={avatar} alt={translate('auth.avatar')} className="h-full w-full object-cover" /> : (username.trim().slice(0, 2).toUpperCase() || '??')}
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <label className="relay-button-secondary relative inline-flex min-h-11 cursor-pointer items-center px-3 text-sm">
                  {preparingAvatar ? translate('auth.saving') : translate('auth.uploadAvatar')}
                  <input type="file" accept="image/png,image/jpeg,image/webp" aria-label={translate('auth.uploadAvatar')}
                    className="absolute inset-0 w-full cursor-pointer opacity-0" disabled={savingProfile || preparingAvatar}
                    onChange={async event => {
                      const file = event.currentTarget.files?.[0]; event.currentTarget.value = '';
                      if (!file) return;
                      setPreparingAvatar(true); setProfileError(null); setProfileMessage(null);
                      try { setAvatar(await resizeAvatar(file)); }
                      catch { setProfileError({ at: 'avatar', message: translate('auth.avatarInvalid') }); }
                      finally { setPreparingAvatar(false); }
                    }} />
                </label>
                {avatar && <button type="button" className="relay-button-secondary min-h-11 px-3 text-sm" disabled={savingProfile || preparingAvatar}
                  onClick={() => { setAvatar(null); setProfileError(null); setProfileMessage(null); }}>{translate('auth.removeAvatar')}</button>}
                {profileError?.at === 'avatar' && <ErrorMark message={profileError.message} />}
                <p className="w-full text-xs text-[var(--theme-fg-muted)]">{translate('auth.avatarHint')}</p>
              </div>
            </div>
            <label className="block text-sm text-[var(--theme-fg-soft)]">
              {translate("auth.username")}<input
                autoComplete="username"
                className="relay-input mt-2 min-h-11 w-full disabled:cursor-wait disabled:opacity-60"
                disabled={savingProfile}
                name="username"
                onChange={(event) => {
                  setUsername(event.target.value);
                  setProfileError(null);
                  setProfileMessage(null);
                }}
                required
                value={username}
              />
            </label>
            {profileMessage ? (
              <Notice>{profileMessage}</Notice>
            ) : null}
            <div className="flex items-center gap-2">
              <button
                className="relay-button-primary inline-flex h-11 items-center gap-2"
                disabled={savingProfile || preparingAvatar || !username.trim() || !profileDirty}
                type="submit"
              >
                <Save aria-hidden="true" className="h-4 w-4" />
                {savingProfile ? translate("auth.saving") : translate("auth.saveProfile")}
              </button>
              {profileError?.at === 'save' && <ErrorMark message={profileError.message} />}
            </div>
          </form>
        </div>
      </section>

      <AccountSecurityPanel />
      <RelaySecurityPanel />

    </div>
  );
}

export function RelayAccountPage() {
  const { locale: i18nLocale } = useI18n();
  return (
    <div className="product-page !max-w-3xl" data-page="account">
      <ProductHeader title={translate('auth.accountSettings')} backHref="/relay-devices" backLabel={translate('auth.devices')} />
      <p className="my-5 text-sm text-[var(--theme-fg-muted)]">{translate('auth.manageTheIdentityAndPasswordUsedTo')}</p>
      <div className="py-2">
        <RelayAccountSettingsPanel />
      </div>
    </div>
  );
}

/** A saved profile is confirmed in place; failures are marks after their control. */
function Notice({ children }: { children: React.ReactNode }) {
  return (
    <div
      aria-live="polite"
      className="rounded-lg bg-[var(--status-success-bg)] px-3 py-2 text-sm text-[var(--status-success-fg)]"
      role="status"
    >
      {children}
    </div>
  );
}
