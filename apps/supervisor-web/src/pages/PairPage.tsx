import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import {
  enableRelayMode,
  fetchRelayPortal,
  fetchRelaySession,
  request,
} from '../lib/api';
import {
  completePairing,
  preparePairing,
  takePairingOffer,
  type PairingOffer,
} from '../lib/ownerPairing';
import { currentOwnerSession, OwnerTrustError } from '../lib/ownerSession';
import type { DeviceCertificate } from '../lib/ownerTrustCrypto';
import {
  clearTransportKeyCache,
  finishPairingInspection,
} from '../lib/relayTransportCrypto';
import {
  probeDeviceEncryption,
  getTransportStatus,
} from '../lib/relayTransport';
import { OwnerUnlockButton } from '../components/OwnerUnlockPrompt';
import { securityError } from '../lib/relaySecurity';

export function PairPage() {
  useI18n();
  const [input] = useState(() => {
    try {
      return { offer: takePairingOffer() };
    } catch (error) {
      return { error: securityError(error) };
    }
  });
  const [ready, setReady] =
    useState<Awaited<ReturnType<typeof preparePairing>>>();
  const [state, setState] = useState<
    | 'loading'
    | 'signin'
    | 'ready'
    | 'busy'
    | 'upload'
    | 'unlock'
    | 'success'
    | 'error'
  >('loading');
  const [error, setError] = useState<string>();
  const [certificate, setCertificate] = useState<DeviceCertificate>();
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let active = true;
    const offer = input.offer;
    if (!offer) {
      setState('error');
      setError(input.error);
      return;
    }
    setState('loading');
    setError(undefined);
    void (async () => {
      enableRelayMode();
      const session = await fetchRelaySession();
      if (!active) return;
      if (!session.authenticated || session.user?.role === 'admin') {
        setState('signin');
        return;
      }
      const portal = await fetchRelayPortal();
      if (
        !portal.devices.some(
          (d) => d.id === offer.deviceId && d.ownerUserId === portal.user.id,
        )
      )
        throw new OwnerTrustError('ownerIdentityMismatch');
      const preparation = await preparePairing(offer);
      if (active) {
        setReady(preparation);
        setState('ready');
      }
    })().catch((e: unknown) => {
      if (active) {
        setError(securityError(e));
        setState('error');
      }
    });
    return () => {
      active = false;
      finishPairingInspection(offer.deviceId);
    };
  }, [input, attempt]);
  async function connected(offer: PairingOffer) {
    clearTransportKeyCache(offer.deviceId);
    navigator.serviceWorker?.controller?.postMessage({
      type: 'remote-codex-reset-transport',
      deviceId: offer.deviceId,
    });
    await probeDeviceEncryption(offer.deviceId);
    if (getTransportStatus(offer.deviceId)?.state !== 'encrypted')
      throw new OwnerTrustError('ownerTrustFailed');
    setState('success');
    setError(undefined);
  }
  async function upload(saved: DeviceCertificate, offer: PairingOffer) {
    try {
      await request(
        `/relay/devices/${encodeURIComponent(offer.deviceId)}/certificate`,
        { method: 'PUT', body: JSON.stringify(saved) },
      );
    } catch {
      setError(translate('auth.ownerPairUpload'));
      setState('upload');
      return;
    }
    setState('unlock');
    setError(undefined);
    if (await currentOwnerSession()) {
      try {
        await connected(offer);
      } catch (e) {
        setError(securityError(e));
      }
    }
  }
  return (
    <main className="mx-auto flex min-h-[100dvh] max-w-lg flex-col justify-center space-y-5 p-5">
      <h1 className="text-2xl font-semibold">
        {translate('auth.ownerPairTitle')}
      </h1>
      {state === 'loading' && (
        <p role="status">{translate('auth.ownerPreparing')}</p>
      )}
      {state === 'signin' && (
        <>
          <p>{translate('auth.ownerSignIn')}</p>
          <Link
            className="relay-button-primary px-4 py-2"
            to="/relay-portal"
            state={{ returnTo: '/pair' }}
          >
            {translate('auth.ownerSignInAction')}
          </Link>
        </>
      )}
      {state === 'ready' && (
        <>
          <p>{translate('auth.ownerPairHelp')}</p>
          <code className="break-all text-xs">
            SHA-256 {input.offer?.identity}
          </code>
          <button
            className="relay-button-primary px-4 py-2"
            onClick={() => {
              if (!ready || !input.offer) return;
              const offer = input.offer;
              const proof = ready.start();
              setState('busy');
              void proof
                .then((value) =>
                  completePairing(offer, ready.descriptor, value),
                )
                .then(async (saved) => {
                  setCertificate(saved);
                  await upload(saved, offer);
                })
                .catch((e: unknown) => {
                  setState('error');
                  setError(securityError(e));
                });
            }}
          >
            {translate('auth.ownerPairAction')}
          </button>
        </>
      )}
      {state === 'busy' && (
        <p role="status">{translate('auth.ownerPreparing')}</p>
      )}
      {error && <p role="alert">{error}</p>}
      {state === 'error' && input.offer && (
        <button
          className="relay-button-secondary px-3 py-2"
          onClick={() => setAttempt((a) => a + 1)}
        >
          {translate('auth.ownerRetry')}
        </button>
      )}
      {(state === 'ready' || state === 'error') && (
        <Link to="/relay-account">
          {translate('auth.ownerAccountSecurity')}
        </Link>
      )}
      {state === 'upload' && certificate && input.offer && (
        <button
          className="relay-button-secondary px-3 py-2"
          onClick={() => {
            if (input.offer) {
              setState('busy');
              void upload(certificate, input.offer);
            }
          }}
        >
          {translate('auth.ownerSaveCertificate')}
        </button>
      )}
      {state === 'unlock' && (
        <>
          <p>{translate('auth.ownerPairSession')}</p>
          <OwnerUnlockButton
            onUnlocked={async () => {
              if (input.offer) await connected(input.offer);
            }}
          />
        </>
      )}
      {state === 'success' && (
        <>
          <p role="status">{translate('auth.ownerPairSuccess')}</p>
          <Link
            className="relay-button-primary px-4 py-2"
            to={`/devices/${encodeURIComponent(input.offer?.deviceId ?? '')}/workspaces`}
          >
            {translate('auth.ownerOpenDevice')}
          </Link>
        </>
      )}
    </main>
  );
}
