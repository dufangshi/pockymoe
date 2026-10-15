import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import type { RelayDeviceDto } from '@pockymoe/shared';
import {
  accountRevocations,
  boundDeviceIdentities,
  currentAccount,
  requireUnlock,
  verifyDeviceCertificate,
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
  canSwitchLocalEnrollmentToRemote,
  switchLocalEnrollmentToRemote,
  runRemoteProtection,
  updateAndProtectDevice,
  deviceProtectionProgress,
  clearResolvedAccountMismatch,
} from './remoteDeviceProtection';
import { clearTransportKeyCache } from './relayTransportCrypto';

vi.mock('./accountVault', () => ({
  accountRevocations: vi.fn(),
  boundDeviceIdentities: vi.fn(),
  currentAccount: vi.fn(),
  requireUnlock: vi.fn(),
  verifyDeviceCertificate: vi.fn(),
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
  vi.mocked(verifyDeviceCertificate).mockResolvedValue(true);
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
      { deviceId: 'bound', identityKey: 'identity' },
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
      phase: 'certificate-missing',
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
  it.each([true, false])(
    'ignores unsigned foreign-account hints for a pinned device with a verifying certificate (bound=%s)',
    async (boundHint) => {
      const certificate = {
        version: 1 as const,
        deviceId: 'pinned',
        accountKeyId: account.accountKeyId,
        identityKey: 'previous',
        createdAt: 1,
        signature: 'signed',
      };
      vi.mocked(boundDeviceIdentities).mockResolvedValue([
        { deviceId: 'pinned', identityKey: 'previous' },
      ]);
      vi.mocked(inspectEnrollmentDevice).mockResolvedValue({
        identityKey: 'previous',
        certificate,
        trust: {
          bound: boundHint,
          strict: boundHint,
          accountKeyId: 'other-account',
        },
      });
      await protectDevices([device('pinned')]);
      expect(await deviceProtectionProgress('pinned')).toEqual({
        phase: 'already-bound',
      });
      expect(verifyDeviceCertificate).toHaveBeenCalledExactlyOnceWith(
        certificate,
        account,
        'pinned',
        'previous',
      );
      expect(createPendingEnrollment).not.toHaveBeenCalled();
      expect(deviceTrustRequest).not.toHaveBeenCalled();
    },
  );
  it.each([
    null,
    {
      version: 1 as const,
      deviceId: 'pinned-invalid',
      accountKeyId: 'other-account',
      identityKey: 'previous',
      createdAt: 1,
      signature: 'forged',
    },
  ])(
    'shows certificate missing for a pinned device with a missing or invalid certificate, never reset advice',
    async (certificate) => {
      vi.mocked(boundDeviceIdentities).mockResolvedValue([
        { deviceId: 'pinned-invalid', identityKey: 'previous' },
      ]);
      vi.mocked(verifyDeviceCertificate).mockResolvedValue(false);
      vi.mocked(inspectEnrollmentDevice).mockResolvedValue({
        identityKey: 'previous',
        certificate,
        trust: { bound: true, strict: true, accountKeyId: 'other-account' },
      });
      await protectDevices([device('pinned-invalid')]);
      expect(await deviceProtectionProgress('pinned-invalid')).toEqual({
        phase: 'certificate-missing',
      });
      expect(createPendingEnrollment).not.toHaveBeenCalled();
      expect(deviceTrustRequest).not.toHaveBeenCalled();
    },
  );
  it('shows identity changed for a pinned device with a changed descriptor identity', async () => {
    vi.mocked(boundDeviceIdentities).mockResolvedValue([
      { deviceId: 'pinned-changed', identityKey: 'previous' },
    ]);
    vi.mocked(inspectEnrollmentDevice).mockResolvedValue({
      identityKey: 'foreign',
      trust: { bound: true, strict: true, accountKeyId: 'other-account' },
    });
    await protectDevices([device('pinned-changed')]);
    expect(await deviceProtectionProgress('pinned-changed')).toEqual({
      phase: 'identity-changed',
    });
    expect(createPendingEnrollment).not.toHaveBeenCalled();
    expect(deviceTrustRequest).not.toHaveBeenCalled();
  });
  it.each([
    ['account_mismatch', 'certificate-missing'],
    ['accountCertificateMissing', 'certificate-missing'],
    ['accountDeviceIdentityChanged', 'identity-changed'],
  ] as const)(
    'keeps pinned inspection errors %s on the existing trust-error path',
    async (code, phase) => {
      vi.mocked(boundDeviceIdentities).mockResolvedValue([
        { deviceId: 'pinned-inspection-error', identityKey: 'previous' },
      ]);
      vi.mocked(inspectEnrollmentDevice).mockRejectedValue(
        new DeviceTrustError(code, 403),
      );
      await protectDevices([device('pinned-inspection-error')]);
      expect(await deviceProtectionProgress('pinned-inspection-error')).toEqual(
        { phase },
      );
      expect(deviceTrustRequest).not.toHaveBeenCalled();
      expect(createPendingEnrollment).not.toHaveBeenCalled();
    },
  );
  it('does not bind when another tab claims local enrollment between the read and atomic create', async () => {
    const local = {
      deviceId: 'concurrent-local',
      userId: account.userId,
      accountKeyId: account.accountKeyId,
      enrollSecret: 'local-only-secret',
      createdAt: Date.now(),
    };
    vi.mocked(createPendingEnrollment).mockImplementationOnce(async () => {
      records.set(local.deviceId, local);
      return local;
    });
    supported.add(local.deviceId);
    await protectDevices([device(local.deviceId)]);
    expect(createPendingEnrollment).toHaveBeenCalledExactlyOnceWith(
      local.deviceId,
      account,
      expect.any(Number),
      { phase: 'queued' },
      null,
    );
    expect(deviceTrustRequest).not.toHaveBeenCalled();
    expect(inspectEnrollmentDevice).not.toHaveBeenCalled();
    expect(pending(local.deviceId)).toEqual(local);
    expect(await deviceProtectionProgress(local.deviceId)).toEqual({
      phase: 'local-command-pending',
    });
  });
  it.each([true, false])(
    'leaves a pending local-command enrollment untouched when Protect all runs (online=%s)',
    async (connected) => {
      const local = {
        deviceId: 'local',
        userId: account.userId,
        accountKeyId: account.accountKeyId,
        enrollSecret: 'local-only-secret',
        createdAt: Date.now(),
      };
      records.set('local', local);
      supported.add('local');
      supported.add('other');
      await protectDevices([device('local', connected), device('other')]);
      expect(pending('local')).toEqual(local);
      expect(await deviceProtectionProgress('local')).toEqual({
        phase: 'local-command-pending',
      });
      expect(deviceTrustRequest).toHaveBeenCalledTimes(1);
      expect(deviceTrustRequest).toHaveBeenCalledWith(
        'other',
        'bind',
        'POST',
        expect.anything(),
      );
      expect(inspectEnrollmentDevice).not.toHaveBeenCalledWith('local');
      expect(completeDeviceEnrollment).toHaveBeenCalledTimes(1);
      expect(completeDeviceEnrollment).not.toHaveBeenCalledWith(
        expect.objectContaining({ deviceId: 'local' }),
        true,
      );
    },
  );
  it('offers the management update, posts it once on approval, and waits for fresh capability after reconnect', async () => {
    await protectDevices([device('upgrade')]);
    expect(request).not.toHaveBeenCalled();
    await Promise.all([
      updateAndProtectDevice('upgrade'),
      updateAndProtectDevice('upgrade'),
    ]);
    expect(request).toHaveBeenCalledExactlyOnceWith(
      'relay/devices/upgrade/api/management/supervisor/update',
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

describe('explicit local-to-remote enrollment switch', () => {
  const id = 'switch-local';
  function localPending() {
    const p = {
      deviceId: id,
      userId: account.userId,
      accountKeyId: account.accountKeyId,
      enrollSecret: 'discarded-local-secret',
      createdAt: Date.now(),
    };
    records.set(id, p);
    supported.add(id);
    return p;
  }
  it('offers switching only an owned online unbound capable device with a pending local enrollment', async () => {
    localPending();
    expect(await canSwitchLocalEnrollmentToRemote(device(id))).toBe(true);
    expect(deviceTrustRequest).not.toHaveBeenCalled();
    expect(requireUnlock).not.toHaveBeenCalled();
  });
  it.each([
    'offline',
    'bound',
    'pinned',
    'old-runtime',
    'no-pending',
    'remote-pending',
    'shared',
    'revoked',
    'certificate',
    'inspection-offline',
  ])('does not offer or perform switching for %s', async (reason) => {
    const local = localPending();
    let candidate = device(id);
    if (reason === 'offline') candidate = device(id, false);
    if (reason === 'shared') candidate = device(id, true, 'another-user');
    if (reason === 'bound') bound.add(id);
    if (reason === 'pinned')
      vi.mocked(boundDeviceIdentities).mockResolvedValue([
        { deviceId: id, identityKey: 'previous' },
      ]);
    if (reason === 'old-runtime') supported.delete(id);
    if (reason === 'no-pending') records.delete(id);
    if (reason === 'remote-pending')
      records.set(id, { ...local, remoteProtection: { phase: 'queued' } });
    if (reason === 'revoked')
      vi.mocked(accountRevocations).mockResolvedValue([
        {
          version: 1,
          accountKeyId: account.accountKeyId,
          kind: 'device',
          subjectId: id,
          revokedAt: 1,
          signature: 'sig',
        },
      ]);
    if (reason === 'certificate')
      vi.mocked(inspectEnrollmentDevice).mockResolvedValue({
        identityKey: 'identity',
        certificate: {
          version: 1,
          deviceId: id,
          accountKeyId: account.accountKeyId,
          identityKey: 'identity',
          createdAt: 1,
          signature: 'sig',
        },
        trust: { bound: false, strict: false, capabilities: ['remote-bind'] },
      });
    if (reason === 'inspection-offline')
      vi.mocked(inspectEnrollmentDevice).mockRejectedValue(
        new DeviceTrustError('device_offline', 503),
      );
    expect(await canSwitchLocalEnrollmentToRemote(candidate)).toBe(false);
    await expect(switchLocalEnrollmentToRemote(candidate)).rejects.toThrow();
    expect(deviceTrustRequest).not.toHaveBeenCalled();
    expect(createPendingEnrollment).not.toHaveBeenCalled();
    if (!['revoked', 'no-pending', 'remote-pending'].includes(reason))
      expect(pending(id)).toEqual(local);
  });
  it('deletes the local enrollment, mints a new secret and completes normal protection with one bind unlock', async () => {
    const local = localPending();
    const create = vi.mocked(createPendingEnrollment).getMockImplementation()!;
    vi.mocked(createPendingEnrollment).mockImplementation(async (...args) => {
      expect(records.has(id)).toBe(false);
      return create(...args);
    });
    await switchLocalEnrollmentToRemote(device(id));
    expect(requireUnlock).toHaveBeenCalledExactlyOnceWith('bind-device');
    expect(createPendingEnrollment).toHaveBeenCalledExactlyOnceWith(
      id,
      account,
      expect.any(Number),
      { phase: 'queued' },
      null,
    );
    expect(deviceTrustRequest).toHaveBeenCalledExactlyOnceWith(
      id,
      'bind',
      'POST',
      {
        accountPublicKey: account.accountPublicKey,
        enrollSecret: 'test-secret-switch-local',
        webOrigin: location.origin,
      },
    );
    for (const call of vi.mocked(deviceTrustRequest).mock.calls)
      expect(call[3]).not.toHaveProperty('enrollSecret', local.enrollSecret);
    expect(completeDeviceEnrollment).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({
        enrollSecret: 'test-secret-switch-local',
        remoteProtection: { phase: 'certifying', bindStarted: true },
      }),
      true,
    );
    expect(records.has(id)).toBe(false);
    expect(await deviceProtectionProgress(id)).toEqual({ phase: 'complete' });
    expect(await runRemoteProtection(local, true)).toBe(false);
  });
  it('rechecks a device that became bound after the button was shown without discarding the local enrollment', async () => {
    const local = localPending();
    expect(await canSwitchLocalEnrollmentToRemote(device(id))).toBe(true);
    bound.add(id);
    await expect(switchLocalEnrollmentToRemote(device(id))).rejects.toThrow();
    expect(pending(id)).toEqual(local);
    expect(deviceTrustRequest).not.toHaveBeenCalled();
  });
  it('does not delete or transmit an enrollment that another view replaced while inspection was pending', async () => {
    const local = localPending();
    const inspect = vi.mocked(inspectEnrollmentDevice).getMockImplementation()!;
    vi.mocked(inspectEnrollmentDevice).mockImplementation(async (deviceId) => {
      records.set(id, { ...local, enrollSecret: 'another-local-secret' });
      return inspect(deviceId);
    });
    await expect(switchLocalEnrollmentToRemote(device(id))).rejects.toThrow();
    expect(pending(id).enrollSecret).toBe('another-local-secret');
    expect(createPendingEnrollment).not.toHaveBeenCalled();
    expect(deviceTrustRequest).not.toHaveBeenCalled();
  });
});

describe('stale account mismatch', () => {
  function failed(id: string) {
    records.set(id, { deviceId: id, userId: account.userId, accountKeyId: account.accountKeyId, createdAt: 1,
      enrollSecret: `secret-${id}`, remoteProtection: { phase: 'error', error: 'account-mismatch' } });
  }
  it('clears the failure once the reset device reports no binding', async () => {
    failed('reset');
    expect(await clearResolvedAccountMismatch(device('reset'))).toBe(true);
    expect(records.has('reset')).toBe(false);
    expect(await deviceProtectionProgress('reset')).toBeUndefined();
  });
  it('keeps the failure while the device is still bound elsewhere, offline, or failed otherwise', async () => {
    failed('elsewhere'); bound.add('elsewhere');
    vi.mocked(inspectEnrollmentDevice).mockResolvedValueOnce({ identityKey: 'identity', trust: { bound: true, strict: true, accountKeyId: 'other' } } as never);
    expect(await clearResolvedAccountMismatch(device('elsewhere'))).toBe(false);
    failed('offline');
    expect(await clearResolvedAccountMismatch(device('offline', false))).toBe(false);
    records.set('update', { ...pending('offline'), deviceId: 'update', remoteProtection: { phase: 'error', error: 'update-failed' } });
    expect(await clearResolvedAccountMismatch(device('update'))).toBe(false);
    expect([...records.keys()].sort()).toEqual(['elsewhere', 'offline', 'update']);
  });
});
