import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ErrorMark } from '@pockymoe/thread-ui';
import { useEffect, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { enableRelayMode, fetchRelaySession, fetchRelayPortal } from '../lib/api';
import { acceptSharedInvite, parseSharedInvite } from '../lib/sharedInvite';
import {
  clearTransportKeyCache,
} from '../lib/relayTransportCrypto';
import { checkSharedInviteDevice } from '../lib/sharedDeviceTrust';
export function SharedInvitePage() {
  useI18n();
  const navigate = useNavigate();
  const [input] = useState(() => {
    try {
      const fragment =
        location.hash ||
        sessionStorage.getItem('pockymoe-pending-invite') ||
        '';
      const invite = parseSharedInvite(fragment);
      sessionStorage.setItem('pockymoe-pending-invite', fragment);
      return { invite };
    } catch (error) {
      return {
        error:
          error instanceof Error
            ? error.message
            : translate('sharing.inviteInvalid'),
      };
    }
  });
  // The page has nothing else to show after a failure: a short label and the
  // reason behind a mark; only a failed session read can be retried.
  const [error, setError] = useState<{ message: string; retry?: boolean } | undefined>(
    input.error ? { message: input.error } : undefined,
  );
  const [attempt, setAttempt] = useState(0);
  const [signin, setSignin] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [deviceName, setDeviceName] = useState('');
  const [ready, setReady] = useState(false);
  useEffect(() => {
    if (!input.invite) return;
    let active = true;
    enableRelayMode();
    void (async () => {
      const session = await fetchRelaySession();
      if (!active) return;
      if (!session.authenticated) {
        setSignin(true);
        return;
      }
      const portal = await fetchRelayPortal().catch(() => null);
      if (!active) return;
      const id = input.invite!.deviceId;
      const received = [
        ...(portal?.sharedWithMe ?? []),
        ...(portal?.sharedDevicesWithMe ?? []),
        ...(portal?.sharedThreadsWithMe ?? []),
      ];
      setDeviceName(portal?.devices.find((d) => d.id === id)?.name ??
        received.find((r) => r.deviceId === id)?.deviceName ?? '');
      setReady(true);
    })().catch((error: unknown) => {
      if (active) setError({ message: error instanceof Error ? error.message : translate('sharing.inviteInvalid'), retry: true });
    });
    return () => { active = false; };
  }, [input, attempt]);
  useEffect(() => {
    let active = true;
    void (async () => {
      if (!input.invite || !confirmed) return;
      enableRelayMode();
      // Pin only after the recipient explicitly reviews the device and identity.
      // The fragment never enters an API request or returnTo parameter.
      const target = await acceptSharedInvite(input.invite);
      if (!active) return;
      clearTransportKeyCache(input.invite.deviceId);
      navigator.serviceWorker?.controller?.postMessage({
        type: 'remote-codex-reset-transport',
        deviceId: input.invite.deviceId,
      });
      // Check the signed descriptor before navigating. No browser registration is
      // needed for this identity check; unlock happens on the destination page.
      await checkSharedInviteDevice(input.invite.deviceId, input.invite.threadId, input.invite.workspaceId);
      if (active) {
        sessionStorage.removeItem('pockymoe-pending-invite');
        navigate(target, { replace: true });
      }
    })().catch((e: unknown) => {
      if (active)
        setError({
          message: e instanceof Error ? e.message : translate('sharing.inviteInvalid'),
        });
    });
    return () => {
      active = false;
    };
  }, [input, navigate, confirmed]);
  return (
    <main className="mx-auto max-w-lg space-y-4 p-6">
      <h1 className="text-xl font-semibold">
        {translate('sharing.inviteTitle')}
      </h1>
      {error ? (
        <p className="flex items-center gap-1.5 text-[var(--theme-fg-muted)]">
          {translate('devices.unavailable')}
          <ErrorMark
            message={error.message}
            onRetry={error.retry ? () => { setError(undefined); setAttempt((value) => value + 1); } : undefined}
          />
        </p>
      ) : signin ? (
        <>
          <p>{translate('auth.ownerSignIn')}</p>
          <Link to="/relay-portal" state={{ returnTo: '/shared' }}>
            {translate('auth.ownerSignInAction')}
          </Link>
        </>
      ) : ready && !confirmed && input.invite ? (
        <>
          <p>{translate('sharing.confirmInviteHelp')}</p>
          <p>{translate('sharing.inviteDevice', {
            value1: deviceName
              ? `${deviceName} (${input.invite.deviceId})`
              : input.invite.deviceId,
          })}</p>
          <code className="block break-all">{translate('sharing.inviteOwnerKey', { value1: input.invite.ownerAccountKeyId })}</code>
          <button className="relay-button-primary px-3 py-2" onClick={() => setConfirmed(true)}>
            {translate('sharing.confirmInvite')}
          </button>
        </>
      ) : (
        <p role="status">{translate('auth.ownerPreparing')}</p>
      )}
    </main>
  );
}
