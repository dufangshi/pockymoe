import { useCallback, useEffect, useState } from 'react';
import { OWNER_UNLOCKED_EVENT } from '../../lib/ownerUnlockRecovery';
import { isProtectedSharingDevice } from '../../lib/sharedDeviceTrust';
import {
  deviceGrants,
  recipientAccount,
  recipientVerified,
  reconcileGrants,
  type DeviceGrant,
} from '../../lib/signedSharing';
import type { SharedItem } from './model';

/** What a share still needs from its owner (the checks SignedShareDetails shows). */
export type AttentionState = 'missing' | 'different' | 'changed' | 'unverified';

/** Checks my shares on online protected devices: the device holds a matching
    signed grant and I compared the recipient's safety code. Offline or older
    devices are skipped rather than flagged. */
export function useShareAttention(items: SharedItem[]) {
  const [attention, setAttention] = useState<ReadonlyMap<string, AttentionState>>(() => new Map());
  const [revision, setRevision] = useState(0);
  const refresh = useCallback(() => setRevision(value => value + 1), []);
  useEffect(() => {
    window.addEventListener(OWNER_UNLOCKED_EVENT, refresh);
    return () => window.removeEventListener(OWNER_UNLOCKED_EVENT, refresh);
  }, [refresh]);
  const signature = items.filter(item => item.direction === 'out').map(item => `${item.key}:${item.deviceConnected}`).join('|');
  useEffect(() => {
    let active = true;
    const outgoing = items.filter(item => item.direction === 'out' && item.deviceConnected === true);
    const devices = [...new Set(outgoing.map(item => item.deviceId))];
    void (async () => {
      const found = new Map<string, AttentionState>();
      await Promise.all(devices.map(async deviceId => {
        let grants: DeviceGrant[];
        try {
          if (!(await isProtectedSharingDevice(deviceId))) return;
          grants = await deviceGrants(deviceId);
        } catch {
          return;
        }
        for (const item of outgoing.filter(entry => entry.deviceId === deviceId)) {
          try {
            const { kind, row } = item.source;
            const account = await recipientAccount(kind, row.id);
            const state = reconcileGrants([{ kind, row, account }], grants)[0]?.state;
            if (state === 'missing' || state === 'different' || state === 'changed') found.set(item.key, state);
            else if (!(await recipientVerified(account, row.targetUserId))) found.set(item.key, 'unverified');
          } catch {
            // A row the relay cannot describe is shown without a flag.
          }
        }
      }));
      if (active) setAttention(found);
    })();
    return () => { active = false; };
    // `signature` lists every outgoing share and its device's presence, so the
    // checks rerun only when one of those changes, not on every portal poll.
  }, [signature, revision]);
  return { attention, refresh };
}
