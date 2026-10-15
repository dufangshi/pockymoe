import { useEffect, useState } from 'react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type { RelayDeviceDto } from '@pockymoe/shared';
import type { DeviceProtectionState } from '../lib/deviceEnrollment';
import { ENROLLMENT_CHANGED_EVENT } from '../lib/pendingEnrollmentStore';
import {
  canSwitchLocalEnrollmentToRemote,
  clearResolvedAccountMismatch,
  deviceProtectionProgress,
  REMOTE_PROTECTION_CHANGED_EVENT,
  type ProtectionProgress,
} from '../lib/remoteDeviceProtection';
import { ProtectDevicesButton } from './RemoteProtectionControls';

type StatusKey =
  | 'protected' | 'notProtected' | 'waitingBind' | 'waitingDevice' | 'updateNeeded' | 'updating'
  | 'protecting' | 'needsRepair' | 'identityChanged' | 'otherAccount' | 'updateFailed' | 'failed';
export type Tone = 'ok' | 'neutral' | 'info' | 'warn' | 'error';
export interface StatusView { key: StatusKey; tone: Tone; anchor: string }
const view = (key: StatusKey, tone: Tone, anchor: string): StatusView => ({ key, tone, anchor });

/** One short status for the device row; the details and fixes live behind it. */
export function protectionView(
  progress: ProtectionProgress | undefined,
  protection: DeviceProtectionState,
): StatusView | null {
  if (protection === 'certificate-missing') return view('needsRepair', 'error', 'needs-repair');
  switch (progress?.phase) {
    case 'queued': return view('waitingDevice', 'info', 'waiting-for-device');
    case 'update-required': return view('updateNeeded', 'warn', 'update-needed');
    case 'updating': return view('updating', 'info', 'update-needed');
    case 'binding':
    case 'certifying': return view('protecting', 'info', 'protecting');
    case 'identity-changed': return view('identityChanged', 'error', 'identity-changed');
    case 'certificate-missing': return view('needsRepair', 'error', 'needs-repair');
    case 'local-command-pending': return view('waitingBind', 'info', 'waiting-for-bind');
    case 'error':
      return progress.error === 'account-mismatch' ? view('otherAccount', 'error', 'other-account')
        : progress.error === 'update-failed' ? view('updateFailed', 'error', 'update-needed')
          : view('failed', 'error', 'protection-failed');
  }
  switch (protection) {
    case 'protected': return view('protected', 'ok', 'protected');
    case 'not-protected': return view('notProtected', 'neutral', 'not-protected');
    case 'pending': return view('waitingBind', 'info', 'waiting-for-bind');
    default: return null;
  }
}

/** Durable remote-protection progress for one device, refreshed as it advances. */
export function useRemoteProtection(device: RelayDeviceDto) {
  const [state, setState] = useState<ProtectionProgress>();
  const [canSwitch, setCanSwitch] = useState(false);
  useEffect(() => {
    let active = true,
      generation = 0;
    const update = () => {
      const current = ++generation;
      void deviceProtectionProgress(device.id)
        .then(async (next) => {
          if (!active || current !== generation) return;
          setState(next);
          setCanSwitch(false);
          if (next?.phase === 'local-command-pending' && device.connected) {
            const eligible = await canSwitchLocalEnrollmentToRemote(device);
            if (active && current === generation) setCanSwitch(eligible);
          }
          // A device reset after a mismatch should not keep the old failure.
          if (next?.phase === 'error' && next.error === 'account-mismatch' && device.connected)
            void clearResolvedAccountMismatch(device).catch(() => undefined);
        })
        .catch(() => {
          if (active && current === generation) setState(undefined);
        });
    };
    const events = [
      ENROLLMENT_CHANGED_EVENT,
      REMOTE_PROTECTION_CHANGED_EVENT,
      'pockymoe-account-logout',
    ];
    update();
    for (const event of events) window.addEventListener(event, update);
    return () => {
      active = false;
      for (const event of events) window.removeEventListener(event, update);
    };
  }, [device.id, device.connected, device.ownerUserId]);
  return { state, canSwitch };
}

/** The current remote-protection step as one line, for the binding dialog. */
export function RemoteProtectionProgress({ device }: { device: RelayDeviceDto }) {
  useI18n();
  const { state, canSwitch } = useRemoteProtection(device);
  const status = state ? protectionView(state, 'unknown') : null;
  if (!status) return null;
  return (
    <>
      <p role={status.tone === 'error' ? 'alert' : 'status'}>
        <span className="font-medium">{translate(`devices.state.${status.key}` as const)}</span>
        {' · '}
        {translate(`devices.detail.${status.key}` as const)}
      </p>
      {state?.phase === 'local-command-pending' && device.connected && canSwitch && (
        <ProtectDevicesButton devices={[device]} replaceLocal />
      )}
    </>
  );
}
