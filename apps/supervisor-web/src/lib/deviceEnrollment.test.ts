// @vitest-environment node
import { webcrypto } from 'node:crypto';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  currentAccount,
  requireUnlock,
  signingInput,
  verifyDeviceCertificate,
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
vi.mock('./accountVault', async (original) => ({
  ...(await original<typeof import('./accountVault')>()),
  currentAccount: vi.fn(),
  requireUnlock: vi.fn(),
  verifyDeviceCertificate: vi.fn(),
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
    expect(update.mock.calls[0]![0](['existing', deviceId])).toEqual([
      'existing',
      deviceId,
    ]);
    expect(clearTransportKeyCache).toHaveBeenCalledWith(deviceId);
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
    expect(deviceTrustRequest).not.toHaveBeenCalled();
    expect(data.has(deviceId)).toBe(false);
  });
  it('retains pending after vault CAS failure, then resumes without re-installing', async () => {
    update.mockRejectedValueOnce(new Error('vault_conflict'));
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
  it('does not remove local trust when the relay refuses revocation', async () => {
    relayFetch.mockResolvedValue(Response.json({}, { status: 403 }));
    await expect(revokeDevice(deviceId)).rejects.toThrow();
    expect(update).not.toHaveBeenCalled();
    expect(data.has(deviceId)).toBe(true);
  });
});
