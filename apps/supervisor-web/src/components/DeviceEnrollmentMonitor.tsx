import { useEffect, useState } from 'react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { currentAccount } from '../lib/accountVault';
import { fetchRelayPortal } from '../lib/api';
import {
  completeDeviceEnrollment,
  deviceIdentityFingerprint,
  ENROLLMENT_CHANGED_EVENT,
  listPendingEnrollments,
  EnrollmentIdentityChangeError,
  type PendingEnrollment,
} from '../lib/deviceEnrollment';
import { FormDialog } from './FormDialog';
import {
  clearTransportKeyCache,
  type TransportStatus,
} from '../lib/relayTransportCrypto';

const POLL_INTERVAL_MS = 15_000;
const MAX_RETRY_MS = 120_000;
const enrollmentKey = (pending: PendingEnrollment) =>
  JSON.stringify([
    pending.userId,
    pending.accountKeyId,
    pending.deviceId,
    pending.createdAt,
    pending.enrollSecret,
  ]);
interface EnrollmentProgress {
  key: string;
  failures: number;
  nextAttemptAt: number;
  connected?: boolean;
  paused?: boolean;
  completed?: boolean;
  wakeAfterAttempt?: boolean;
}
interface ReplacementPrompt {
  enrollment: PendingEnrollment;
  change: EnrollmentIdentityChangeError;
  deviceName: string;
  previousFingerprint: string | null;
  newFingerprint: string;
}
async function replacementPrompt(
  enrollment: PendingEnrollment,
  change: EnrollmentIdentityChangeError,
  deviceName: string,
): Promise<ReplacementPrompt> {
  const [previousFingerprint, newFingerprint] = await Promise.all([
    change.previousIdentityKey
      ? deviceIdentityFingerprint(change.previousIdentityKey)
      : Promise.resolve(null),
    deviceIdentityFingerprint(change.identityKey),
  ]);
  return { enrollment, change, deviceName, previousFingerprint, newFingerprint };
}

/** Runs while the browser is open, including when it is viewing a thread. */
export function DeviceEnrollmentMonitor() {
  useI18n();
  const [error, setError] = useState<string | null>(null);
  const [generation, setGeneration] = useState(0);
  const [replacement, setReplacement] = useState<ReplacementPrompt | null>(null);
  const [confirming, setConfirming] = useState(false);
  async function confirmReplacement() {
    if (!replacement || confirming) return;
    setConfirming(true);
    try {
      const account = await currentAccount();
      const live = await listPendingEnrollments(account);
      if (
        !account ||
        !live.some(
          (p) => enrollmentKey(p) === enrollmentKey(replacement.enrollment),
        )
      ) {
        setReplacement(null);
        return;
      }
      await completeDeviceEnrollment(replacement.enrollment, true, {
        previousIdentityKey: replacement.change.previousIdentityKey,
        identityKey: replacement.change.identityKey,
      });
      setReplacement(null);
      setError(null);
      setGeneration((g) => g + 1);
    } catch (caught) {
      if (caught instanceof EnrollmentIdentityChangeError)
        setReplacement(
          await replacementPrompt(
            replacement.enrollment,
            caught,
            replacement.deviceName,
          ),
        );
      else {
        setReplacement(null);
        setError(
          caught instanceof Error
            ? caught.message
            : translate('devices.enrollmentFailed'),
        );
      }
    } finally {
      setConfirming(false);
    }
  }
  useEffect(() => {
    let stopped = false,
      running = false,
      queued = false;
    const progress = new Map<string, EnrollmentProgress>();
    const hints = new Map<string, string>();
    async function tick() {
      if (running || stopped || document.visibilityState === 'hidden') return;
      running = true;
      try {
        const account = await currentAccount();
        const pending = account ? await listPendingEnrollments(account) : [];
        if (stopped) return;
        const live = new Map(
          pending.map((p) => [p.deviceId, enrollmentKey(p)]),
        );
        for (const [id, state] of progress) {
          if (live.get(id) !== state.key) {
            progress.delete(id);
            hints.delete(id);
          }
        }
        for (const enrollment of pending) {
          if (!progress.has(enrollment.deviceId))
            progress.set(enrollment.deviceId, {
              key: enrollmentKey(enrollment),
              failures: 0,
              nextAttemptAt: 0,
            });
        }
        setReplacement((previous) =>
          previous &&
          live.get(previous.enrollment.deviceId) ===
            enrollmentKey(previous.enrollment)
            ? previous
            : null,
        );
        if (![...progress.values()].some((s) => s.paused)) setError(null);
        if (
          !pending.some((p) => {
            const s = progress.get(p.deviceId)!;
            return !s.paused && !s.completed;
          })
        )
          return;
        const portal = await fetchRelayPortal();
        for (const enrollment of pending) {
          if (stopped) break;
          const state = progress.get(enrollment.deviceId)!;
          const device = portal.devices.find(
            (d) => d.id === enrollment.deviceId,
          );
          const connected = !!device?.connected;
          if (state.connected === false && connected) {
            state.failures = 0;
            state.nextAttemptAt = 0;
          }
          state.connected = connected;
          if (
            !connected ||
            state.paused ||
            state.completed ||
            Date.now() < state.nextAttemptAt
          )
            continue;
          try {
            state.wakeAfterAttempt = false;
            state.completed = await completeDeviceEnrollment(enrollment, true);
          } catch (caught) {
            if (caught instanceof EnrollmentIdentityChangeError) {
              state.paused = true;
              try {
                const prompt = await replacementPrompt(
                  enrollment,
                  caught,
                  device?.name || enrollment.deviceId,
                );
                if (!stopped) setReplacement(prompt);
              } catch (failure) {
                if (!stopped)
                  setError(
                    failure instanceof Error
                      ? failure.message
                      : translate('devices.enrollmentMismatch'),
                  );
              }
              continue;
            }
            // enrollment_missing means the local bind command has not run yet.
            const code = (caught as { code?: string } | null)?.code;
            if (code === 'browser_registration_required')
              clearTransportKeyCache(enrollment.deviceId);
            if (
              caught instanceof TypeError ||
              (caught instanceof Error &&
                (caught as Error & { status?: number }).status !== undefined &&
                (caught as Error & { status: number }).status >= 500) ||
              (caught instanceof Error &&
                [
                  'device_offline',
                  'device_unresponsive',
                  'enrollment_missing',
                  'browser_registration_required',
                ].includes((caught as Error & { code?: string }).code ?? ''))
            ) {
              state.failures = state.wakeAfterAttempt ? 0 : state.failures + 1;
              state.nextAttemptAt =
                state.wakeAfterAttempt
                  ? 0
                  : Date.now() +
                    Math.min(
                      POLL_INTERVAL_MS * 2 ** Math.min(state.failures - 1, 3),
                      MAX_RETRY_MS,
                    );
              continue;
            }
            state.paused = true;
            if (!stopped)
              setError(
                caught instanceof Error
                  ? caught.message
                  : translate('devices.enrollmentFailed'),
              );
          }
        }
      } catch {
        /* Account not set up or relay unreachable; retry later. */
      } finally {
        running = false;
        if (queued && !stopped) {
          queued = false;
          void tick();
        }
      }
    }
    const update = () => {
      if (running) queued = true;
      void tick();
    };
    const wake = () => {
      for (const state of progress.values()) {
        state.failures = 0;
        state.nextAttemptAt = 0;
        if (running) state.wakeAfterAttempt = true;
      }
      update();
    };
    const trustChanged = (event: Event) => {
      const status = (event as CustomEvent<TransportStatus>).detail;
      const state = status && progress.get(status.deviceId);
      if (!state || state.completed) return;
      // A registration rejection is retried with backoff, not a new binding
      // hint. Alternating encrypted/error statuses must not create a retry loop.
      if (status.state === 'unlock-required' || status.state === 'error') return;
      const hint = JSON.stringify([
        status.state,
        status.bound,
        status.accountKeyId,
        status.identityKey,
        status.certificateVerified,
        status.role,
      ]);
      if (hints.get(status.deviceId) === hint) return;
      hints.set(status.deviceId, hint);
      clearTransportKeyCache(status.deviceId);
      state.failures = 0;
      state.nextAttemptAt = 0;
      if (running) state.wakeAfterAttempt = true;
      update();
    };
    void tick();
    const interval = window.setInterval(update, POLL_INTERVAL_MS);
    window.addEventListener(ENROLLMENT_CHANGED_EVENT, update);
    window.addEventListener('pockymoe-account-revocations-changed', update);
    window.addEventListener('remote-codex-transport', trustChanged);
    window.addEventListener('online', wake);
    document.addEventListener('visibilitychange', wake);
    return () => {
      stopped = true;
      window.clearInterval(interval);
      window.removeEventListener(ENROLLMENT_CHANGED_EVENT, update);
      window.removeEventListener(
        'pockymoe-account-revocations-changed',
        update,
      );
      window.removeEventListener('remote-codex-transport', trustChanged);
      window.removeEventListener('online', wake);
      document.removeEventListener('visibilitychange', wake);
    };
  }, [generation]);
  return (
    <>
      {replacement && (
        <FormDialog
          title={translate('devices.confirmIdentityReplacement')}
          description={translate('devices.identityReplacementHelp')}
          busy={confirming}
          onClose={() => {
            setReplacement(null);
            setError(translate('devices.confirmIdentityReplacement'));
          }}
        >
          <p className="text-sm font-medium">{replacement.deviceName}</p>
          <p className="break-all font-mono text-xs">
            {replacement.enrollment.deviceId}
          </p>
          <dl className="space-y-2 text-sm">
            <dt>{translate('devices.previousIdentity')}</dt>
            <dd className="break-all font-mono text-xs">
              {replacement.previousFingerprint ||
                translate('devices.identityUnavailable')}
            </dd>
            <dt>{translate('devices.newIdentity')}</dt>
            <dd className="break-all font-mono text-xs">
              {replacement.newFingerprint}
            </dd>
          </dl>
          <p className="mt-3 text-sm">
            {translate('devices.identityReplacementCompare')}
          </p>
          <button
            className="relay-button-primary mt-4 px-3 py-2"
            disabled={confirming}
            onClick={() => void confirmReplacement()}
          >
            {translate('devices.replaceIdentity')}
          </button>
        </FormDialog>
      )}
      {error ? (
        <div
          role="alert"
          className="fixed bottom-4 left-4 z-[90] max-w-md rounded border border-[var(--theme-border)] bg-[var(--theme-panel)] p-4"
        >
          <p>{error}</p>
          <button
            className="relay-button-secondary mt-2 px-3 py-2"
            onClick={() => {
              setError(null);
              setGeneration((g) => g + 1);
            }}
          >
            {translate('devices.retryEnrollment')}
          </button>
        </div>
      ) : null}
    </>
  );
}
