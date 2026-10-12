// @vitest-environment node
import { webcrypto } from 'node:crypto';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  currentAccount,
  requireUnlock,
  signingInput,
  verifyDeviceCertificate,
  boundDeviceIdentities,
  confirmBoundDeviceIdentity,
} from './accountVault';
import { deviceTrustRequest } from './deviceTrust';
import * as transport from './relayTransportCrypto';
const inspectEnrollmentDevice = (
  transport as unknown as { inspectEnrollmentDevice: ReturnType<typeof vi.fn> }
).inspectEnrollmentDevice;
const clearTransportKeyCache = transport.clearTransportKeyCache;
import {
  completeDeviceEnrollment,
  createPendingEnrollment,
  deletePendingEnrollment,
  deviceProtectionState,
  ENROLLMENT_LIFETIME_MS,
  listPendingEnrollments,
  prepareDeviceEnrollment,
  revokeDevice,
  verifyEnrollmentMac,
  type PendingEnrollment,
  type EnrollmentResponse,
} from './deviceEnrollment';
import vectors from './fixtures/deviceEnrollmentVectors.json';
import { accountStore } from './accountStore';
import { sharingTrustStore } from './sharingTrustStore';
vi.mock('./accountStore', () => ({ accountStore: vi.fn() }));
vi.mock('./sharingTrustStore', () => ({ sharingTrustStore: vi.fn() }));
vi.mock('./accountVault', async (original) => ({
  ...(await original<typeof import('./accountVault')>()),
  currentAccount: vi.fn(),
  requireUnlock: vi.fn(),
  verifyDeviceCertificate: vi.fn(),
  boundDeviceIdentities: vi.fn(),
  confirmBoundDeviceIdentity: vi.fn(),
}));
vi.mock('./deviceTrust', () => ({ deviceTrustRequest: vi.fn() }));
vi.mock('./relayTransportCrypto', () => ({
  inspectEnrollmentDevice: vi.fn(),
  clearTransportKeyCache: vi.fn(),
  reportTransport: vi.fn(),
}));
vi.mock('./relayOrigin', () => ({
  relayUrl: (path: string) => `https://relay.test${path}`,
}));
const sign = vi.fn(),
  update = vi.fn(),
  relayFetch = vi.fn();
const data = new Map<string, PendingEnrollment>();
const trustData = new Map<string, unknown>();
const sharingData = new Map<string, unknown>();
const account = vectors.account;
const deviceId = vectors.deviceCertificate.deviceId;
let pending: PendingEnrollment;
const enrollment: EnrollmentResponse = {
  deviceId,
  identityKey: vectors.deviceCertificate.identityKey,
  accountKeyId: account.accountKeyId,
  mac: vectors.enrollment.mac,
};

beforeEach(() => {
  vi.clearAllMocks();
  data.clear();
  trustData.clear();
  sharingData.clear();
  for (const [mock, records] of [
    [accountStore, trustData],
    [sharingTrustStore, sharingData],
  ] as const) {
    vi.mocked(mock).mockImplementation(
      async (key: string, ...write: [unknown?]) => {
        if (!write.length) return records.get(key) as never;
        if (write[0] === null) records.delete(key);
        else records.set(key, structuredClone(write[0]));
        return undefined;
      },
    );
  }
  vi.stubGlobal('crypto', webcrypto);
  vi.stubGlobal('fetch', relayFetch);
  vi.stubGlobal('indexedDB', {
    open: () => {
      const open: Record<string, any> = {};
      open.result = {
        close: vi.fn(),
        transaction: () => {
          const tx: Record<string, any> = {};
          tx.objectStore = () => ({
            put: (record: PendingEnrollment) =>
              data.set(record.deviceId, structuredClone(record)),
            delete: (id: string) => data.delete(id),
            getAll: () => {
              const get: Record<string, any> = {};
              queueMicrotask(() => {
                get.result = structuredClone([...data.values()]);
                get.onsuccess?.();
              });
              return get;
            },
          });
          setTimeout(() => tx.oncomplete?.(), 0);
          return tx;
        },
      };
      queueMicrotask(() => open.onsuccess?.());
      return open;
    },
  });
  pending = {
    deviceId,
    userId: account.userId,
    accountKeyId: account.accountKeyId,
    enrollSecret: vectors.enrollment.enrollSecret,
    createdAt: Date.now(),
  };
  data.set(deviceId, pending);
  sign.mockResolvedValue('signature');
  update.mockResolvedValue(undefined);
  relayFetch.mockResolvedValue(Response.json({}));
  vi.mocked(currentAccount).mockResolvedValue(account);
  vi.mocked(boundDeviceIdentities).mockResolvedValue([]);
  vi.mocked(confirmBoundDeviceIdentity).mockResolvedValue(undefined);
  vi.mocked(requireUnlock).mockResolvedValue({
    ...account,
    sign,
    updateBoundDevices: update,
  });
  vi.mocked(verifyDeviceCertificate).mockImplementation(
    async (certificate, a, id, identity) =>
      certificate.accountKeyId === a.accountKeyId &&
      certificate.deviceId === id &&
      certificate.identityKey === identity &&
      ['signature', vectors.deviceCertificate.signature].includes(
        certificate.signature,
      ),
  );
  vi.mocked(inspectEnrollmentDevice as any).mockResolvedValue({
    identityKey: enrollment.identityKey,
    certificate: null,
    trust: { bound: true, strict: true, accountKeyId: account.accountKeyId },
  });
  vi.mocked(deviceTrustRequest).mockImplementation(
    async (_id, path, _method, body) => {
      if (path === 'enrollment') return enrollment as never;
      return { certificate: body } as never;
    },
  );
});

describe('pending enrollment lifecycle', () => {
  it('stores independent random 32-byte secrets, lists by account and deletes', async () => {
    data.clear();
    const a = await createPendingEnrollment('a', account);
    const b = await createPendingEnrollment('b', account);
    expect(Buffer.from(a.enrollSecret, 'base64url')).toHaveLength(32);
    expect(a.enrollSecret).not.toBe(b.enrollSecret);
    expect(await listPendingEnrollments(account)).toHaveLength(2);
    expect(
      await listPendingEnrollments({ ...account, userId: 'someone-else' }),
    ).toEqual([]);
    expect(
      await listPendingEnrollments({ ...account, accountKeyId: 'other-key' }),
    ).toEqual([]);
    await deletePendingEnrollment('a');
    expect(
      (await listPendingEnrollments(account)).map((p) => p.deviceId),
    ).toEqual(['b']);
  });
  it('expires and physically deletes at seven days, including other accounts', async () => {
    data.clear();
    const now = Date.now();
    await createPendingEnrollment('old', account, now - ENROLLMENT_LIFETIME_MS);
    await createPendingEnrollment(
      'live',
      account,
      now - ENROLLMENT_LIFETIME_MS + 1,
    );
    expect(
      (await listPendingEnrollments(account, now)).map((p) => p.deviceId),
    ).toEqual(['live']);
    expect(data.has('old')).toBe(false);
  });
  it('requires a bind unlock and reuses a live command secret', async () => {
    const binding = await prepareDeviceEnrollment(deviceId);
    expect(binding.enrollSecret).toBe(pending.enrollSecret);
    expect(binding.accountPublicKey).toBe(account.accountPublicKey);
    expect(requireUnlock).toHaveBeenCalledWith('bind-device');
  });
});
describe('enrollment protocol', () => {
  it('passes the authoritative HMAC vector and rejects its negative identity vector', async () => {
    expect(await verifyEnrollmentMac(pending, enrollment)).toBe(true);
    expect(
      await verifyEnrollmentMac(pending, {
        ...enrollment,
        mac: vectors.negative.enrollmentMacWrongIdentity,
      }),
    ).toBe(false);
    expect(
      await verifyEnrollmentMac(pending, { ...enrollment, mac: 'malformed!' }),
    ).toBe(false);
  });
  it.each([
    { mac: vectors.negative.enrollmentMacWrongIdentity },
    { deviceId: 'device-2' },
    { identityKey: 'wrong-identity' },
    { accountKeyId: 'wrong-account' },
  ])(
    'refuses mismatching enrollment before prompting or signing (%j)',
    async (fields) => {
      vi.mocked(deviceTrustRequest).mockResolvedValue({
        ...enrollment,
        ...fields,
      });
      await expect(completeDeviceEnrollment(pending, true)).rejects.toThrow();
      expect(requireUnlock).not.toHaveBeenCalled();
      expect(sign).not.toHaveBeenCalled();
      expect(relayFetch).not.toHaveBeenCalled();
      expect(data.has(deviceId)).toBe(true);
    },
  );
  it('installs, uploads, updates the vault and then deletes pending', async () => {
    expect(await completeDeviceEnrollment(pending, true)).toBe(true);
    expect(requireUnlock).toHaveBeenCalledWith('certify-device');
    const certificate = vi.mocked(deviceTrustRequest).mock
      .calls[1]![3] as typeof vectors.deviceCertificate;
    expect(sign).toHaveBeenCalledWith(signingInput.device(certificate));
    expect(deviceTrustRequest).toHaveBeenCalledWith(
      deviceId,
      'device-certificate',
      'PUT',
      certificate,
    );
    expect(data.has(deviceId)).toBe(false);
    expect(relayFetch).toHaveBeenCalledWith(
      `https://relay.test/relay/devices/${deviceId}/certificate`,
      expect.objectContaining({
        method: 'PUT',
        body: JSON.stringify(certificate),
      }),
    );
    expect(confirmBoundDeviceIdentity).toHaveBeenCalledWith(
      deviceId,
      enrollment.identityKey,
      null,
    );
    expect(clearTransportKeyCache).toHaveBeenCalledWith(deviceId);
  });
  it('requires explicit replacement confirmation before signing or installing a new identity', async () => {
    vi.mocked(boundDeviceIdentities).mockResolvedValue([
      { deviceId, identityKey: 'previous-identity' },
    ]);
    await expect(completeDeviceEnrollment(pending, true)).rejects.toMatchObject(
      {
        deviceId,
        previousIdentityKey: 'previous-identity',
        identityKey: enrollment.identityKey,
      },
    );
    expect(requireUnlock).not.toHaveBeenCalled();
    expect(sign).not.toHaveBeenCalled();
    expect(confirmBoundDeviceIdentity).not.toHaveBeenCalled();
    expect(deviceTrustRequest).not.toHaveBeenCalledWith(
      deviceId,
      'device-certificate',
      'PUT',
      expect.anything(),
    );
    expect(data.has(deviceId)).toBe(true);
  });
  it('does not publish a Protected transport status from certificate installation alone', async () => {
    await completeDeviceEnrollment(pending, true);
    expect(transport.reportTransport).not.toHaveBeenCalled();
  });
  it('accepts an explicitly confirmed replacement only for the exact old/new identity pair', async () => {
    vi.mocked(boundDeviceIdentities).mockResolvedValue([
      { deviceId, identityKey: 'old-identity' },
    ]);
    await expect(
      completeDeviceEnrollment(pending, true, {
        previousIdentityKey: 'stale-identity',
        identityKey: enrollment.identityKey,
      }),
    ).rejects.toMatchObject({ previousIdentityKey: 'old-identity' });
    expect(confirmBoundDeviceIdentity).not.toHaveBeenCalled();
    expect(
      await completeDeviceEnrollment(pending, true, {
        previousIdentityKey: 'old-identity',
        identityKey: enrollment.identityKey,
      }),
    ).toBe(true);
    expect(confirmBoundDeviceIdentity).toHaveBeenCalledWith(
      deviceId,
      enrollment.identityKey,
      'old-identity',
    );
    expect(
      vi.mocked(confirmBoundDeviceIdentity).mock.invocationCallOrder[0],
    ).toBeLessThan(sign.mock.invocationCallOrder[0]!);
  });
  it('verifies the enrollment MAC before presenting a replacement confirmation', async () => {
    vi.mocked(boundDeviceIdentities).mockResolvedValue([
      { deviceId, identityKey: 'old-identity' },
    ]);
    vi.mocked(deviceTrustRequest).mockResolvedValue({
      ...enrollment,
      mac: vectors.negative.enrollmentMacWrongIdentity,
    });
    await expect(
      completeDeviceEnrollment(pending, true),
    ).rejects.not.toMatchObject({ previousIdentityKey: 'old-identity' });
    expect(confirmBoundDeviceIdentity).not.toHaveBeenCalled();
    expect(sign).not.toHaveBeenCalled();
  });
  it('refuses a concurrent vault identity change after explicit confirmation', async () => {
    vi.mocked(boundDeviceIdentities).mockResolvedValue([
      { deviceId, identityKey: 'old-identity' },
    ]);
    vi.mocked(confirmBoundDeviceIdentity).mockRejectedValueOnce(
      new Error('accountCertificateInvalid'),
    );
    await expect(
      completeDeviceEnrollment(pending, true, {
        previousIdentityKey: 'old-identity',
        identityKey: enrollment.identityKey,
      }),
    ).rejects.toThrow('accountCertificateInvalid');
    expect(sign).not.toHaveBeenCalled();
    expect(relayFetch).not.toHaveBeenCalled();
    expect(data.has(deviceId)).toBe(true);
  });
  it('never re-enrolls a locally revoked device even if a vault CAS left its pending enrollment', async () => {
    update.mockRejectedValueOnce(new Error('vault_conflict'));
    await expect(revokeDevice(deviceId)).rejects.toThrow('vault_conflict');
    expect(await completeDeviceEnrollment(pending, true)).toBe(false);
    expect(inspectEnrollmentDevice).not.toHaveBeenCalled();
    expect(data.has(deviceId)).toBe(false);
  });
  it('retries when an offline device comes online', async () => {
    expect(await completeDeviceEnrollment(pending, false)).toBe(false);
    expect(deviceTrustRequest).not.toHaveBeenCalled();
    expect(data.has(deviceId)).toBe(true);
    expect(await completeDeviceEnrollment(pending, true)).toBe(true);
  });
  it('retains pending on network failure and resumes after the device consumed its secret', async () => {
    relayFetch.mockResolvedValueOnce(Response.json({}, { status: 503 }));
    await expect(completeDeviceEnrollment(pending, true)).rejects.toThrow();
    expect(data.get(deviceId)?.certificate).toBeDefined();
    expect(update).not.toHaveBeenCalled();
    const certificate = data.get(deviceId)!.certificate!;
    vi.mocked(inspectEnrollmentDevice as any).mockResolvedValue({
      identityKey: enrollment.identityKey,
      certificate,
    });
    vi.mocked(deviceTrustRequest).mockClear();
    expect(await completeDeviceEnrollment(data.get(deviceId)!, true)).toBe(
      true,
    );
    expect(deviceTrustRequest).toHaveBeenCalledExactlyOnceWith(
      deviceId,
      'trust',
    );
    expect(data.has(deviceId)).toBe(false);
  });
  it('retains pending after vault CAS failure, then resumes without re-installing', async () => {
    vi.mocked(confirmBoundDeviceIdentity).mockRejectedValueOnce(
      new Error('vault_conflict'),
    );
    await expect(completeDeviceEnrollment(pending, true)).rejects.toThrow(
      'vault_conflict',
    );
    const certificate = data.get(deviceId)!.certificate!;
    vi.mocked(inspectEnrollmentDevice as any).mockResolvedValue({
      identityKey: enrollment.identityKey,
      certificate,
    });
    expect(await completeDeviceEnrollment(data.get(deviceId)!, true)).toBe(
      true,
    );
  });
  it('refuses wrong device/identity certificate responses', async () => {
    vi.mocked(deviceTrustRequest).mockImplementation(async (_id, path) =>
      path === 'enrollment'
        ? (enrollment as never)
        : ({ certificate: vectors.negative.deviceCertWrongIdentity } as never),
    );
    await expect(completeDeviceEnrollment(pending, true)).rejects.toThrow();
    expect(update).not.toHaveBeenCalled();
    expect(relayFetch).not.toHaveBeenCalled();
  });
  it('does not enroll a different signed-in account or key', async () => {
    vi.mocked(currentAccount).mockResolvedValue({
      ...account,
      userId: 'other-user',
    });
    expect(await completeDeviceEnrollment(pending, true)).toBe(false);
    expect(deviceTrustRequest).not.toHaveBeenCalled();
  });
});
describe('device protection states', () => {
  const descriptor = {
    identityKey: enrollment.identityKey,
    certificate: vectors.deviceCertificate as any,
  };
  it('uses a verified certificate for Protected', async () => {
    expect(
      await deviceProtectionState(
        deviceId,
        descriptor,
        [],
        [deviceId],
        account,
      ),
    ).toBe('protected');
  });
  it('shows waiting, unbound relay trust and missing certificates without downgrade', async () => {
    expect(
      await deviceProtectionState(deviceId, null, [pending], [], account),
    ).toBe('pending');
    expect(
      await deviceProtectionState(
        deviceId,
        { identityKey: enrollment.identityKey },
        [],
        [],
        account,
      ),
    ).toBe('not-protected');
    expect(
      await deviceProtectionState(
        deviceId,
        null,
        [pending],
        [deviceId],
        account,
      ),
    ).toBe('certificate-missing');
    expect(await deviceProtectionState(deviceId, null, [], [], account)).toBe(
      'unknown',
    );
  });
  it.each([
    vectors.negative.deviceCertWrongDevice,
    vectors.negative.deviceCertWrongIdentity,
  ])('refuses invalid identity certificates', async (certificate) => {
    expect(
      await deviceProtectionState(
        deviceId,
        { ...descriptor, certificate: certificate as any },
        [],
        [deviceId],
        account,
      ),
    ).toBe('certificate-missing');
  });
});
describe('remove device', () => {
  it.each([false, true])(
    'persists local revocation and deletes certificate caches before vault CAS (fails: %s)',
    async (fails) => {
      const certificateKey = `device-certificate:${account.accountKeyId}:${deviceId}`;
      const protectedKey = `protected-device:${account.accountKeyId}:${deviceId}`;
      const otherKey = `device-certificate:${account.accountKeyId}:other`;
      trustData.set(certificateKey, vectors.deviceCertificate);
      trustData.set(protectedKey, true);
      sharingData.set(certificateKey, vectors.deviceCertificate);
      sharingData.set(otherKey, { other: true });
      const oldRevocation = {
        kind: 'device',
        subjectId: 'other',
        accountKeyId: account.accountKeyId,
      };
      trustData.set(`revocations:${account.userId}`, [oldRevocation]);
      update.mockImplementationOnce(async () => {
        expect(trustData.has(certificateKey)).toBe(false);
        expect(trustData.has(protectedKey)).toBe(false);
        expect(sharingData.has(certificateKey)).toBe(false);
        expect(trustData.get(`revocations:${account.userId}`)).toEqual([
          oldRevocation,
          expect.objectContaining({
            kind: 'device',
            subjectId: deviceId,
            signature: 'signature',
          }),
        ]);
        if (fails) throw new Error('vault_conflict');
      });
      if (fails)
        await expect(revokeDevice(deviceId)).rejects.toThrow('vault_conflict');
      else await revokeDevice(deviceId);
      expect(sharingData.get(otherKey)).toEqual({ other: true });
    },
  );
  it('signs device revocation, posts it and removes only that bound id', async () => {
    await revokeDevice(deviceId);
    expect(requireUnlock).toHaveBeenCalledWith('revoke');
    const { revocation } = JSON.parse(relayFetch.mock.calls[0]![1].body);
    expect(revocation).toMatchObject({
      version: 1,
      kind: 'device',
      subjectId: deviceId,
      accountKeyId: account.accountKeyId,
      signature: 'signature',
    });
    expect(sign).toHaveBeenCalledWith(signingInput.revocation(revocation));
    expect(relayFetch.mock.calls[0]![0]).toBe(
      'https://relay.test/relay/account/revocations',
    );
    expect(update.mock.calls[0]![0]([deviceId, 'other'])).toEqual(['other']);
    expect(data.has(deviceId)).toBe(false);
  });
  it('invalidates trust after the signed tombstone is posted even when the vault CAS fails', async () => {
    const dispatchEvent = vi.fn();
    vi.stubGlobal('window', { dispatchEvent });
    update.mockRejectedValueOnce(new Error('vault_conflict'));
    try {
      await expect(revokeDevice(deviceId)).rejects.toThrow('vault_conflict');
      expect(clearTransportKeyCache).toHaveBeenCalledWith(deviceId);
      const event = dispatchEvent.mock.calls[0]![0] as CustomEvent;
      expect(event.type).toBe('pockymoe-account-revocations-changed');
      expect(event.detail.revocation).toMatchObject({
        kind: 'device',
        subjectId: deviceId,
      });
      expect(data.has(deviceId)).toBe(true);
    } finally {
      vi.stubGlobal('window', undefined);
    }
  });
  it('does not remove local trust when the relay refuses revocation', async () => {
    relayFetch.mockResolvedValue(Response.json({}, { status: 403 }));
    await expect(revokeDevice(deviceId)).rejects.toThrow();
    expect(update).not.toHaveBeenCalled();
    expect(data.has(deviceId)).toBe(true);
  });
});
