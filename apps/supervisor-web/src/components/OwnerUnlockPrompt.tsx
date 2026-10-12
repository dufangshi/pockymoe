import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Link, useLocation } from 'react-router-dom';
import { prepareOwnerUnlock } from '../lib/ownerSession';
import {
  clearTransportKeyCache,
  type TransportStatus,
} from '../lib/relayTransportCrypto';
import {
  getTransportStatus,
  probeDeviceEncryption,
} from '../lib/relayTransport';
import { useDialogLifecycle } from './useDialogLifecycle';
import { securityError } from '../lib/relaySecurity';

export function OwnerUnlockButton({
  onUnlocked,
  renew = false,
  credentialId,
}: {
  onUnlocked: () => void | Promise<void>;
  renew?: boolean;
  credentialId?: string | string[];
}) {
  useI18n();
  const [unlock, setUnlock] =
    useState<Awaited<ReturnType<typeof prepareOwnerUnlock>>>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let active = true;
    setUnlock(undefined);
    setError(undefined);
    prepareOwnerUnlock(credentialId)
      .then((start) => {
        if (active) setUnlock(() => start);
      })
      .catch((e: unknown) => {
        if (active) setError(securityError(e));
      });
    return () => {
      active = false;
    };
  }, [attempt, credentialId]);
  return (
    <div className="space-y-2">
      {error && (
        <p role="alert" className="text-sm">
          {error}
        </p>
      )}
      {error ? (
        <button
          className="relay-button-secondary px-3 py-2"
          onClick={() => setAttempt((a) => a + 1)}
        >
          {translate('auth.ownerRetry')}
        </button>
      ) : (
        <button
          className="relay-button-primary px-3 py-2"
          disabled={!unlock || busy}
          onClick={() => {
            if (!unlock) return;
            // Start WebAuthn in the gesture, before the first await.
            const result = unlock();
            setBusy(true);
            void result
              .then(onUnlocked)
              .catch((e: unknown) => setError(securityError(e)))
              .finally(() => setBusy(false));
          }}
        >
          {!unlock
            ? translate('auth.ownerPreparing')
            : translate(renew ? 'auth.ownerRenew' : 'auth.ownerUnlock')}
        </button>
      )}
    </div>
  );
}
export function OwnerUnlockPrompt() {
  useI18n();
  const location = useLocation();
  const [devices, setDevices] = useState<string[]>([]);
  const [credentialId, setCredentialId] = useState<string | string[]>();
  const [dismissed, setDismissed] = useState(false);
  const dialog = useRef<HTMLElement>(null);
  useDialogLifecycle({
    containerRef: dialog,
    open: !!devices.length && !dismissed && location.pathname !== '/pair',
    onClose: () => setDismissed(true),
  });
  useEffect(() => {
    const onStatus = (event: Event) => {
      const status = (event as CustomEvent<TransportStatus>).detail;
      if (event.type === 'remote-codex-owner-unlock-request')
        setDismissed(false);
      if (status.state === 'unlock-required') {
        if (status.ownerCredentialId) setCredentialId(status.ownerCredentialId);
        setDevices((current) =>
          current.includes(status.deviceId)
            ? current
            : [...current, status.deviceId],
        );
      }
    };
    window.addEventListener('remote-codex-transport', onStatus);
    window.addEventListener('remote-codex-owner-unlock-request', onStatus);
    return () => {
      window.removeEventListener('remote-codex-transport', onStatus);
      window.removeEventListener('remote-codex-owner-unlock-request', onStatus);
    };
  }, []);
  useEffect(() => {
    setDismissed(false);
  }, [location.pathname]);
  // Pairing has its own explicit verification and completion prompts.
  if (!devices.length || dismissed || location.pathname === '/pair')
    return null;
  return createPortal(
    <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/40 p-4">
      <section
        ref={dialog}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby="owner-unlock-title"
        className="w-full max-w-md space-y-4 rounded-xl border border-[var(--theme-border)] bg-[var(--theme-panel)] p-5 shadow-xl"
      >
        <h2 id="owner-unlock-title" className="text-lg font-semibold">
          {translate('auth.ownerUnlockTitle')}
        </h2>
        <p>{translate('auth.ownerUnlockHelp')}</p>
        <OwnerUnlockButton
          {...(credentialId ? { credentialId } : {})}
          onUnlocked={async () => {
            // Reconnect only after verification; application mutations are never replayed.
            for (const id of devices) {
              clearTransportKeyCache(id);
              navigator.serviceWorker?.controller?.postMessage({
                type: 'remote-codex-reset-transport',
                deviceId: id,
              });
              await probeDeviceEncryption(id);
              if (getTransportStatus(id)?.state !== 'encrypted')
                throw new Error(translate('auth.ownerTrustFailed'));
            }
            setDevices([]);
            setCredentialId(undefined);
            window.dispatchEvent(new Event('remote-codex-owner-unlocked'));
          }}
        />
        <div className="flex flex-wrap items-center gap-4 text-sm">
          <Link to="/relay-account" onClick={() => setDismissed(true)}>
            {translate('auth.ownerAccountSecurity')}
          </Link>
          <button onClick={() => setDismissed(true)}>
            {translate('auth.ownerLater')}
          </button>
        </div>
      </section>
    </div>,
    document.body,
  );
}
