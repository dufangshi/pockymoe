import { useEffect, useState } from 'react';
import {
  accountRevocations,
  boundDevices,
  currentAccount,
} from './accountVault';
import {
  deviceProtectionState,
  ENROLLMENT_CHANGED_EVENT,
  listPendingEnrollments,
  type DeviceProtectionState,
} from './deviceEnrollment';
import { type TransportStatus } from './relayTransportCrypto';

export function useDeviceProtection(
  deviceId?: string,
  status?: TransportStatus,
) {
  const [state, setState] = useState<DeviceProtectionState>('unknown');
  useEffect(() => {
    let active = true;
    async function refresh() {
      if (!deviceId) return;
      const account = await currentAccount();
      const [bound, pending, revocations] = await Promise.all([
        boundDevices(),
        listPendingEnrollments(account),
        accountRevocations(),
      ]);
      const revoked = revocations.some(
        (r) =>
          r.accountKeyId === account?.accountKeyId &&
          r.kind === 'device' &&
          r.subjectId === deviceId,
      );
      const next =
        revoked ||
        (status?.state === 'identity-changed' && status.certificateVerified)
          ? 'certificate-missing'
          : account &&
              status?.deviceId === deviceId &&
              status.state === 'encrypted' &&
              status.certificateVerified === true &&
              status.role === 'owner' &&
              status.accountKeyId === account.accountKeyId
            ? 'protected'
            : await deviceProtectionState(
                deviceId,
                null,
                pending,
                bound,
                account,
              );
      if (active)
        setState(
          next === 'unknown' &&
            (status?.state === 'legacy' ||
              (['encrypted', 'identity-changed'].includes(
                status?.state ?? '',
              ) &&
                status?.paired === false))
            ? 'not-protected'
            : next,
        );
    }
    void refresh().catch(() => {
      if (active) setState('unknown');
    });
    const update = () => {
      void refresh().catch(() => {
        if (active) setState('unknown');
      });
    };
    const events = [
      ENROLLMENT_CHANGED_EVENT,
      'pockymoe-account-revocations-changed',
      'pockymoe-account-revocations',
      'pockymoe-account-logout',
    ];
    for (const event of events) window.addEventListener(event, update);
    return () => {
      active = false;
      for (const event of events) window.removeEventListener(event, update);
    };
  }, [deviceId, status]);
  return state;
}
export const protectionLabel = (state: DeviceProtectionState) =>
  (
    ({
      protected: 'devices.protected',
      'not-protected': 'devices.notProtected',
      pending: 'devices.waitingForConnection',
      'certificate-missing': 'devices.certificateMissing',
      unknown: 'devices.protectionUnknown',
    }) as const
  )[state];
