import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import type { RelayDeviceDto } from '@pockymoe/shared';
import {
  accountRevocations,
  boundDeviceIdentities,
  currentAccount,
  requireUnlock,
} from './accountVault';
import { request } from './api';
import {
  createPendingEnrollment,
  completeDeviceEnrollment,
  inspectEnrollmentDevice,
  EnrollmentIdentityChangeError,
} from './deviceEnrollment';
import { deviceTrustRequest, DeviceTrustError } from './deviceTrust';
import {
  listPendingEnrollments,
  savePending,
  type PendingEnrollment,
} from './pendingEnrollmentStore';
import {
  protectDevices,
  runRemoteProtection,
  updateAndProtectDevice,
  deviceProtectionProgress,
} from './remoteDeviceProtection';
import { clearTransportKeyCache } from './relayTransportCrypto';

vi.mock('./accountVault', () => ({
  accountRevocations: vi.fn(),
  boundDeviceIdentities: vi.fn(),
  currentAccount: vi.fn(),
  requireUnlock: vi.fn(),
}));
vi.mock('./api', () => ({ request: vi.fn() }));
vi.mock('./deviceEnrollment', () => ({
  createPendingEnrollment: vi.fn(),
  completeDeviceEnrollment: vi.fn(),
  inspectEnrollmentDevice: vi.fn(),
  EnrollmentIdentityChangeError: class extends Error {},
}));
vi.mock('./deviceTrust', async (original) => ({
  ...(await original<object>()),
  deviceTrustRequest: vi.fn(),
}));
vi.mock('./relayTransportCrypto', () => ({ clearTransportKeyCache: vi.fn() }));
vi.mock('./pendingEnrollmentStore', () => ({
  listPendingEnrollments: vi.fn(),
  savePending: vi.fn(),
  pendingIsCurrent: async (p: PendingEnrollment) =>
    records.get(p.deviceId)?.enrollSecret === p.enrollSecret,
  deletePendingEnrollment: async (id: string, expected: PendingEnrollment) => {
    if (records.get(id)?.enrollSecret !== expected.enrollSecret) return false;
    records.delete(id);
    return true;
  },
}));
const account = {
  userId: 'owner',
  accountKeyId: 'account',
  accountPublicKey: 'public-key',
};
const records = new Map<string, PendingEnrollment>();
const bound = new Set<string>();
const supported = new Set<string>();
function device(
  id: string,
  connected = true,
  ownerUserId = account.userId,
): RelayDeviceDto {
  return {
    id,
    connected,
    ownerUserId,
    name: id,
    token: null,
    tokenPreview: '',
    connectedAt: null,
    lastHeartbeatAt: null,
    createdAt: '',
  };
}
function pending(id: string) {
  return records.get(id)!;
}
beforeEach(() => {
  vi.resetAllMocks();
  records.clear();
  bound.clear();
  supported.clear();
  vi.mocked(requireUnlock).mockResolvedValue(account as never);
  vi.mocked(currentAccount).mockResolvedValue(account);
  vi.mocked(accountRevocations).mockResolvedValue([]);
  vi.mocked(boundDeviceIdentities).mockResolvedValue([]);
  vi.mocked(listPendingEnrollments).mockImplementation(async () =>
    [...records.values()].map((p) => structuredClone(p)),
  );
  vi.mocked(savePending).mockImplementation(
    async (p, expected, expectedPhase) => {
      if (
        expected &&
        records.get(p.deviceId)?.enrollSecret !== expected.enrollSecret
      )
        return false;
      if (
        expectedPhase !== undefined &&
        records.get(p.deviceId)?.remoteProtection?.phase !== expectedPhase
      )
        return false;
      records.set(p.deviceId, structuredClone(p));
      return true;
    },
  );
  vi.mocked(createPendingEnrollment).mockImplementation(
    async (id, a, now, remoteProtection) => {
      const p = {
        deviceId: id,
        userId: a.userId,
        accountKeyId: a.accountKeyId,
        createdAt: now!,
        enrollSecret: `test-secret-${id}`,
        ...(remoteProtection ? { remoteProtection } : {}),
      };
      records.set(id, p);
      return structuredClone(p);
    },
  );
  vi.mocked(inspectEnrollmentDevice).mockImplementation(async (id) => ({
    identityKey: 'identity',
    trust: {
      bound: bound.has(id),
      strict: bound.has(id),
      accountKeyId: account.accountKeyId,
      ...(supported.has(id) ? { capabilities: ['remote-bind'] } : {}),
    },
  }));
  vi.mocked(deviceTrustRequest).mockImplementation(async (id, path) => {
    if (path === 'bind') bound.add(id);
    return {
      bound: true,
      deviceId: id,
      accountKeyId: account.accountKeyId,
    } as never;
  });
  vi.mocked(completeDeviceEnrollment).mockImplementation(async (p) => {
    records.delete(p.deviceId);
    return true;
  });
  vi.mocked(request).mockResolvedValue({ job: { phase: 'restarting' } });
});
afterEach(() => vi.useRealTimers());

describe('one-click protection batches', () => {
  it('uses one unlock for mixed online/offline/old/bound/mismatched owned devices, and excludes shared devices', async () => {
    supported.add('online');
    supported.add('mismatch');
    vi.mocked(boundDeviceIdentities).mockResolvedValue([
      { deviceId: 'bound', identityKey: 'pinned' },
    ]);
    const inspect = vi.mocked(inspectEnrollmentDevice).getMockImplementation()!;
    vi.mocked(inspectEnrollmentDevice).mockImplementation(async (id) =>
      id === 'mismatch'
        ? {
            identityKey: 'foreign',
            trust: { bound: true, strict: true, accountKeyId: 'other-account' },
          }
        : inspect(id),
    );
    await protectDevices([
      device('online'),
      device('offline', false),
      device('old'),
      device('bound'),
      device('mismatch'),
      device('shared', true, 'other-owner'),
    ]);
    expect(requireUnlock).toHaveBeenCalledExactlyOnceWith('bind-device');
    expect(deviceTrustRequest).toHaveBeenCalledExactlyOnceWith(
      'online',
      'bind',
      'POST',
      {
        accountPublicKey: account.accountPublicKey,
        enrollSecret: 'test-secret-online',
        webOrigin: location.origin,
      },
    );
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(1);
    expect(await deviceProtectionProgress('online')).toEqual({
      phase: 'complete',
    });
    expect(pending('offline').remoteProtection).toEqual({ phase: 'queued' });
    expect(pending('old').remoteProtection).toEqual({
      phase: 'update-required',
    });
    expect(await deviceProtectionProgress('bound')).toEqual({
      phase: 'already-bound',
    });
    expect(pending('mismatch').remoteProtection).toEqual({
      phase: 'error',
      error: 'account-mismatch',
    });
    expect(request).not.toHaveBeenCalled();
    expect(records.has('shared')).toBe(false);
    expect(inspectEnrollmentDevice).not.toHaveBeenCalledWith('offline');
    expect(inspectEnrollmentDevice).toHaveBeenCalledWith('bound');
  });
  it('resumes durable offline intent on reconnect with its original enrollment secret', async () => {
    supported.add('later');
    await protectDevices([device('later', false)]);
    const stored = structuredClone(pending('later'));
    expect(await runRemoteProtection(stored, false)).toBe(false);
    expect(await runRemoteProtection(stored, true)).toBe(true);
    expect(createPendingEnrollment).toHaveBeenCalledTimes(1);
    expect(requireUnlock).toHaveBeenCalledTimes(1);
    expect(deviceTrustRequest).toHaveBeenCalledWith(
      'later',
      'bind',
      'POST',
      expect.objectContaining({ enrollSecret: stored.enrollSecret }),
    );
    expect(records.has('later')).toBe(false);
  });
  it('skips a device already bound to this account even when it has not been locally recorded', async () => {
    bound.add('already-bound');
    await protectDevices([device('already-bound')]);
    expect(await deviceProtectionProgress('already-bound')).toEqual({
      phase: 'already-bound',
    });
    expect(records.has('already-bound')).toBe(false);
    expect(deviceTrustRequest).not.toHaveBeenCalled();
    expect(completeDeviceEnrollment).not.toHaveBeenCalled();
  });
  it('reports a different account key even for a previously pinned device, without rebinding it', async () => {
    vi.mocked(boundDeviceIdentities).mockResolvedValue([
      { deviceId: 'pinned-mismatch', identityKey: 'previous' },
    ]);
    vi.mocked(inspectEnrollmentDevice).mockResolvedValue({
      identityKey: 'foreign',
      trust: { bound: true, strict: true, accountKeyId: 'other-account' },
    });
    await protectDevices([device('pinned-mismatch')]);
    expect(await deviceProtectionProgress('pinned-mismatch')).toEqual({
      phase: 'error',
      error: 'account-mismatch',
    });
    expect(createPendingEnrollment).not.toHaveBeenCalled();
    expect(deviceTrustRequest).not.toHaveBeenCalled();
  });
  it('offers the management update, posts it once on approval, and waits for fresh capability after reconnect', async () => {
    await protectDevices([device('upgrade')]);
    expect(request).not.toHaveBeenCalled();
    await Promise.all([
      updateAndProtectDevice('upgrade'),
      updateAndProtectDevice('upgrade'),
    ]);
    expect(request).toHaveBeenCalledExactlyOnceWith(
      '/relay/devices/upgrade/api/management/supervisor/update',
      { method: 'POST', body: '{}' },
    );
    expect(pending('upgrade').remoteProtection?.phase).toBe('updating');
    await runRemoteProtection(structuredClone(pending('upgrade')), true);
    expect(deviceTrustRequest).not.toHaveBeenCalled();
    expect(
      await runRemoteProtection(structuredClone(pending('upgrade')), false),
    ).toBe(false);
    supported.add('upgrade');
    expect(
      await runRemoteProtection(structuredClone(pending('upgrade')), true),
    ).toBe(true);
    expect(requireUnlock).toHaveBeenCalledTimes(1);
    expect(clearTransportKeyCache).toHaveBeenCalledWith('upgrade');
  });
  it('offers updates for a runtime without encryption, without sending any enrollment secret', async () => {
    vi.mocked(inspectEnrollmentDevice).mockRejectedValueOnce(
      Object.assign(new Error('No key descriptor'), {
        code: 'accountIdentityMismatch',
      }),
    );
    await protectDevices([device('legacy')]);
    expect(pending('legacy').remoteProtection).toEqual({
      phase: 'update-required',
    });
    expect(deviceTrustRequest).not.toHaveBeenCalled();
    expect(request).not.toHaveBeenCalled();
    await updateAndProtectDevice('legacy');
    supported.add('legacy');
    await runRemoteProtection(structuredClone(pending('legacy')), true);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(1);
  });
  it('reports failed or completed updates that still lack the capability and does not update automatically again', async () => {
    vi.useFakeTimers();
    await protectDevices([device('bad-update')]);
    await updateAndProtectDevice('bad-update');
    vi.advanceTimersByTime(15000);
    vi.mocked(request).mockResolvedValue({ job: { phase: 'completed' } });
    await runRemoteProtection(structuredClone(pending('bad-update')), true);
    expect(pending('bad-update').remoteProtection).toEqual({
      phase: 'error',
      error: 'update-failed',
    });
    await runRemoteProtection(structuredClone(pending('bad-update')), true);
    expect(request).toHaveBeenCalledTimes(2);
    expect(deviceTrustRequest).not.toHaveBeenCalled();
  });
  it('reports an update that the management API cannot perform instead of waiting forever', async () => {
    await protectDevices([device('manual-update')]);
    vi.mocked(request).mockResolvedValue({ canUpdate: false });
    await updateAndProtectDevice('manual-update');
    expect(pending('manual-update').remoteProtection).toEqual({
      phase: 'error',
      error: 'update-failed',
    });
    expect(deviceTrustRequest).not.toHaveBeenCalled();
  });
  it('refuses a mismatched bind response before certifying', async () => {
    supported.add('wrong-response');
    vi.mocked(deviceTrustRequest).mockResolvedValue({
      bound: true,
      deviceId: 'wrong-device',
      accountKeyId: account.accountKeyId,
    });
    await protectDevices([device('wrong-response')]);
    expect(pending('wrong-response').remoteProtection).toEqual({
      phase: 'error',
      error: 'account-mismatch',
    });
    expect(completeDeviceEnrollment).not.toHaveBeenCalled();
  });
  it('reports a relay race that binds the device to another account key', async () => {
    supported.add('raced');
    vi.mocked(inspectEnrollmentDevice)
      .mockResolvedValueOnce({
        identityKey: 'identity',
        trust: { bound: false, strict: false, capabilities: ['remote-bind'] },
      })
      .mockResolvedValueOnce({
        identityKey: 'identity',
        trust: { bound: true, strict: true, accountKeyId: 'attacker-account' },
      });
    await protectDevices([device('raced')]);
    expect(pending('raced').remoteProtection?.error).toBe('account-mismatch');
    expect(completeDeviceEnrollment).not.toHaveBeenCalled();
  });
  it('keeps already_bound races pending and certifies after fresh inspection, without resetting or rebinding', async () => {
    supported.add('race');
    vi.mocked(deviceTrustRequest).mockImplementationOnce(async () => {
      bound.add('race');
      throw new DeviceTrustError('already_bound', 409);
    });
    await protectDevices([device('race')]);
    expect(
      await runRemoteProtection(structuredClone(pending('race')), true),
    ).toBe(true);
    expect(deviceTrustRequest).toHaveBeenCalledTimes(1);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(1);
  });
  it('isolates an owner error and continues protecting other devices', async () => {
    supported.add('denied');
    supported.add('good');
    vi.mocked(deviceTrustRequest).mockRejectedValueOnce(
      new DeviceTrustError('owner_required', 403),
    );
    await protectDevices([device('denied'), device('good')]);
    expect(pending('denied').remoteProtection?.error).toBe('failed');
    expect(await deviceProtectionProgress('good')).toEqual({
      phase: 'complete',
    });
  });
  it('retries a lost bind reply by inspecting the now-bound device instead of sending another bind', async () => {
    supported.add('lost-reply');
    vi.mocked(deviceTrustRequest).mockImplementationOnce(async () => {
      bound.add('lost-reply');
      throw new DeviceTrustError('device_offline', 503);
    });
    await protectDevices([device('lost-reply')]);
    expect(
      await runRemoteProtection(structuredClone(pending('lost-reply')), true),
    ).toBe(true);
    expect(deviceTrustRequest).toHaveBeenCalledTimes(1);
  });
  it('preserves post-bind intent when certificate installation succeeds but completion is interrupted', async () => {
    supported.add('partial-certificate');
    vi.mocked(completeDeviceEnrollment).mockImplementationOnce(async (p) => {
      records.set(p.deviceId, {
        ...p,
        certificate: {
          version: 1,
          accountKeyId: p.accountKeyId,
          deviceId: p.deviceId,
          identityKey: 'identity',
          createdAt: 1,
          signature: 'test-signature',
        },
      });
      throw new DeviceTrustError('device_offline', 503);
    });
    await protectDevices([device('partial-certificate')]);
    expect(pending('partial-certificate').remoteProtection).toMatchObject({
      phase: 'certifying',
      bindStarted: true,
    });
    expect(
      await runRemoteProtection(
        structuredClone(pending('partial-certificate')),
        true,
      ),
    ).toBe(true);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(2);
    expect(deviceTrustRequest).toHaveBeenCalledTimes(1);
  });
  it('never binds a removed, revoked, or other-account enrollment', async () => {
    supported.add('removed');
    await protectDevices([device('removed', false)]);
    const stale = structuredClone(pending('removed'));
    records.delete('removed');
    expect(await runRemoteProtection(stale, true)).toBe(false);
    records.set(stale.deviceId, stale);
    vi.mocked(currentAccount).mockResolvedValue({
      ...account,
      accountKeyId: 'changed-account',
    });
    expect(await runRemoteProtection(stale, true)).toBe(false);
    vi.mocked(currentAccount).mockResolvedValue(account);
    vi.mocked(accountRevocations).mockResolvedValue([
      {
        version: 1,
        accountKeyId: account.accountKeyId,
        kind: 'device',
        subjectId: stale.deviceId,
        revokedAt: 0,
        signature: 'sig',
      },
    ]);
    expect(await runRemoteProtection(stale, true)).toBe(false);
    expect(deviceTrustRequest).not.toHaveBeenCalled();
  });
  it('leaves MAC-gated identity replacement to the existing monitor confirmation flow', async () => {
    supported.add('replacement');
    const change = new EnrollmentIdentityChangeError(
      'replacement',
      'old',
      'new',
    );
    vi.mocked(completeDeviceEnrollment).mockRejectedValue(change);
    await protectDevices([device('replacement')]);
    expect(pending('replacement').remoteProtection?.phase).toBe('certifying');
    await expect(
      runRemoteProtection(structuredClone(pending('replacement')), true),
    ).rejects.toBe(change);
  });
});
