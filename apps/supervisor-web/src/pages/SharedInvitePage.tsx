import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { enableRelayMode, fetchRelaySession } from '../lib/api';
import { acceptSharedInvite, parseSharedInvite } from '../lib/sharedInvite';
import {
  clearTransportKeyCache,
  sharingFingerprint,
} from '../lib/relayTransportCrypto';
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
  const [error, setError] = useState(input.error);
  const [signin, setSignin] = useState(false);
  useEffect(() => {
    let active = true;
    void (async () => {
      if (!input.invite) return;
      enableRelayMode();
      // Persist the pin before sign-in or any device handshake. The fragment is
      // never included in an API request or returnTo query parameter.
      const target = await acceptSharedInvite(input.invite);
      const session = await fetchRelaySession();
      if (!active) return;
      if (!session.authenticated) {
        setSignin(true);
        return;
      }
      clearTransportKeyCache(input.invite.deviceId);
      navigator.serviceWorker?.controller?.postMessage({
        type: 'remote-codex-reset-transport',
        deviceId: input.invite.deviceId,
      });
      // Check the signed descriptor before navigating. No passkey assertion is
      // needed for this identity check; unlock happens on the destination page.
      await sharingFingerprint(input.invite.deviceId, input.invite.threadId);
      if (active) {
        sessionStorage.removeItem('pockymoe-pending-invite');
        navigate(target, { replace: true });
      }
    })().catch((e: unknown) => {
      if (active)
        setError(
          e instanceof Error ? e.message : translate('sharing.inviteInvalid'),
        );
    });
    return () => {
      active = false;
    };
  }, [input, navigate]);
  return (
    <main className="mx-auto max-w-lg space-y-4 p-6">
      <h1 className="text-xl font-semibold">
        {translate('sharing.inviteTitle')}
      </h1>
      {error ? (
        <p role="alert">{error}</p>
      ) : signin ? (
        <>
          <p>{translate('auth.ownerSignIn')}</p>
          <Link to="/relay-portal" state={{ returnTo: '/shared' }}>
            {translate('auth.ownerSignInAction')}
          </Link>
        </>
      ) : (
        <p role="status">{translate('auth.ownerPreparing')}</p>
      )}
    </main>
  );
}
