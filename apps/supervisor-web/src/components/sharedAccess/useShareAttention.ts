import { useCallback, useEffect, useRef, useState } from 'react';
import { OWNER_UNLOCKED_EVENT } from '../../lib/ownerUnlockRecovery';
import { isProtectedSharingDevice } from '../../lib/sharedDeviceTrust';
import { useRequireSafetyCode } from '../../lib/sharingPreferences';
import {
  deviceGrants,
  recipientAccount,
  recipientVerified,
  reconcileGrants,
  type DeviceGrant,
} from '../../lib/signedSharing';
import type { SharedItem } from './model';

/** What a share still needs from its owner (the states SignedShareDetails shows). */
export type AttentionState = 'pending' | 'missing' | 'different' | 'changed' | 'unverified';

/** Checks my shares: one waiting for the recipient's safety code; on online
    protected devices, a missing or different signed grant or a changed
    recipient key, and (while the owner keeps the check on) an active share
    whose code nobody checked. Offline or older devices are skipped rather than
    flagged. */
export function useShareAttention(items: SharedItem[]) {
  const [attention, setAttention] = useState<ReadonlyMap<string, AttentionState>>(() => new Map());
  const [revision, setRevision] = useState(0);
  const requireCode = useRequireSafetyCode();
  const refresh = useCallback(() => setRevision(value => value + 1), []);
  useEffect(() => {
    window.addEventListener(OWNER_UNLOCKED_EVENT, refresh);
    return () => window.removeEventListener(OWNER_UNLOCKED_EVENT, refresh);
  }, [refresh]);
  const itemsRef = useRef(items);
  itemsRef.current = items;
  // Every outgoing share with what its checks read, so the checks rerun only
  // when one of those changes, not on every portal refresh.
  const signature = items.filter(item => item.direction === 'out').map(item => {
    const row = item.source.row;
    return [item.key, item.deviceConnected, row.pending === true, row.verifiedAt ?? '', row.threadAccess, row.workspaceAccess, row.expiresAt ?? '',
      'scope' in row ? `${row.scope}:${row.workspaceScope}:${(row.workspaceIds ?? []).join(',')}:${row.canCreateThreads}` : ''].join('/');
  }).join('|');
  useEffect(() => {
    let active = true;
    const outgoing = itemsRef.current.filter(item => item.direction === 'out');
    const found = new Map<string, AttentionState>();
    for (const item of outgoing) if (item.source.row.pending === true) found.set(item.key, 'pending');
    const online = outgoing.filter(item => item.deviceConnected === true && !found.has(item.key));
    const devices = [...new Set(online.map(item => item.deviceId))];
    void (async () => {
      await Promise.all(devices.map(async deviceId => {
        let grants: DeviceGrant[] | null;
        try {
          grants = (await isProtectedSharingDevice(deviceId)) ? await deviceGrants(deviceId) : null;
        } catch {
          return;
        }
        if (!grants) return;
        for (const item of online.filter(entry => entry.deviceId === deviceId)) {
          try {
            const { kind, row } = item.source;
            const account = await recipientAccount(kind, row.id);
            const state = reconcileGrants([{ kind, row, account }], grants)[0]?.state;
            if (state === 'missing' || state === 'different' || state === 'changed') found.set(item.key, state);
            else if (requireCode && !row.verifiedAt && !(await recipientVerified(account, row.targetUserId))) found.set(item.key, 'unverified');
          } catch {
            // A row the relay cannot describe (or a recipient without an
            // account key) is shown without a flag.
          }
        }
      }));
      if (active) setAttention(previous => sameAttention(previous, found) ? previous : found);
    })();
    return () => { active = false; };
  }, [signature, revision, requireCode]);
  return { attention, refresh };
}

function sameAttention(a: ReadonlyMap<string, AttentionState>, b: ReadonlyMap<string, AttentionState>) {
  return a.size === b.size && [...a].every(([key, value]) => b.get(key) === value);
}
