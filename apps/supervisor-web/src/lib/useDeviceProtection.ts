import { useEffect, useState } from 'react';
import { boundDevices, currentAccount } from './accountVault';
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
      const [bound, pending] = await Promise.all([
        boundDevices(),
        listPendingEnrollments(account),
      ]);
      const next =
        status?.state === 'identity-changed' && status.certificateVerified
          ? 'certificate-missing'
          : status?.certificateVerified
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
    window.addEventListener(ENROLLMENT_CHANGED_EVENT, update);
    return () => {
      active = false;
      window.removeEventListener(ENROLLMENT_CHANGED_EVENT, update);
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
