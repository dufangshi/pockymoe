import { useEffect, useState } from 'react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { currentAccount } from '../lib/accountVault';
import { fetchRelayPortal } from '../lib/api';
import {
  completeDeviceEnrollment,
  ENROLLMENT_CHANGED_EVENT,
  listPendingEnrollments,
  EnrollmentIdentityChangeError,
  type PendingEnrollment,
} from '../lib/deviceEnrollment';
import { FormDialog } from './FormDialog';

/** Runs while the browser is open, including when it is viewing a thread. */
export function DeviceEnrollmentMonitor() {
  useI18n();
  const [error, setError] = useState<string | null>(null);
  const [generation, setGeneration] = useState(0);
  const [replacement, setReplacement] = useState<{
    enrollment: PendingEnrollment;
    change: EnrollmentIdentityChangeError;
  } | null>(null);
  const [confirming, setConfirming] = useState(false);
  async function confirmReplacement() {
    if (!replacement || confirming) return;
    setConfirming(true);
    try {
      await completeDeviceEnrollment(replacement.enrollment, true, {
        previousIdentityKey: replacement.change.previousIdentityKey,
        identityKey: replacement.change.identityKey,
      });
      setReplacement(null);
      setError(null);
      setGeneration((g) => g + 1);
    } catch (caught) {
      if (caught instanceof EnrollmentIdentityChangeError)
        setReplacement({ enrollment: replacement.enrollment, change: caught });
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
      running = false;
    const paused = new Set<string>();
    async function tick() {
      if (running || stopped || document.visibilityState === 'hidden') return;
      running = true;
      try {
        const account = await currentAccount();
        if (!account) return;
        const pending = await listPendingEnrollments(account);
        if (!pending.length) return;
        const portal = await fetchRelayPortal();
        for (const enrollment of pending) {
          if (stopped || paused.has(enrollment.deviceId)) continue;
          const device = portal.devices.find(
            (d) => d.id === enrollment.deviceId,
          );
          if (!device?.connected) continue;
          try {
            await completeDeviceEnrollment(enrollment, true);
          } catch (caught) {
            if (caught instanceof EnrollmentIdentityChangeError) {
              paused.add(enrollment.deviceId);
              if (!stopped) setReplacement({ enrollment, change: caught });
              continue;
            }
            // Offline transport errors keep their secret for a later connection.
            if (
              caught instanceof TypeError ||
              (caught instanceof Error &&
                (caught as Error & { status?: number }).status !== undefined &&
                (caught as Error & { status: number }).status >= 500) ||
              (caught instanceof Error &&
                ['device_offline', 'device_unresponsive'].includes(
                  (caught as Error & { code?: string }).code ?? '',
                ))
            )
              continue;
            paused.add(enrollment.deviceId);
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
      }
    }
    const update = () => {
      void tick();
    };
    void tick();
    const interval = window.setInterval(update, 15_000);
    window.addEventListener(ENROLLMENT_CHANGED_EVENT, update);
    document.addEventListener('visibilitychange', update);
    return () => {
      stopped = true;
      window.clearInterval(interval);
      window.removeEventListener(ENROLLMENT_CHANGED_EVENT, update);
      document.removeEventListener('visibilitychange', update);
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
          <p className="break-all font-mono text-xs">
            {replacement.enrollment.deviceId}
          </p>
          <dl className="space-y-2 text-sm">
            <dt>{translate('devices.previousIdentity')}</dt>
            <dd className="break-all font-mono text-xs">
              {replacement.change.previousIdentityKey ||
                translate('devices.identityUnavailable')}
            </dd>
            <dt>{translate('devices.newIdentity')}</dt>
            <dd className="break-all font-mono text-xs">
              {replacement.change.identityKey}
            </dd>
          </dl>
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
