// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from 'vitest';
import vectors from './fixtures/accountKeyVectors.json';
import type { AccountKeyInfo, DeviceCertificate } from './accountVault';
const state = vi.hoisted(() => ({
  account: vi.fn(), bound: vi.fn(), verify: vi.fn(), fetch: vi.fn(),
  store: new Map<string, unknown>(),
}));
vi.mock('./accountVault', () => ({
  currentAccount: state.account,
  boundDevices: state.bound,
  verifyDeviceCertificate: state.verify,
}));
vi.mock('./sharingTrustStore', () => ({
  sharingTrustStore: async (key: string, ...write: unknown[]) => {
    if (write.length) {
      if (write[0] === null) state.store.delete(key);
      else state.store.set(key, structuredClone(write[0]));
    } else return structuredClone(state.store.get(key));
  },
  sharingTrustEntries: async (prefix: string) => [...state.store.entries()].filter(([key]) => key.startsWith(prefix)),
}));
vi.stubGlobal('fetch', state.fetch);
const trust = await import('./sharedDeviceTrust');
const account = { accountKeyId: vectors.account.accountKeyId, accountPublicKey: vectors.account.accountPublicKey };
const certificate = { ...vectors.deviceCertificate.certificate, version: 1 as const };
function raw(value: string) {
  return Uint8Array.from(Buffer.from(value, 'base64url'));
}
beforeEach(() => {
  state.store.clear();
  state.account.mockReset().mockResolvedValue(null);
  state.bound.mockReset().mockResolvedValue([]);
  // accountVault is a parallel dependency. Mock its contract using real vector
  // signatures, so altered certificates cannot pass via a permissive test stub.
  state.verify.mockReset().mockImplementation(async (c: DeviceCertificate, a: AccountKeyInfo, deviceId: string, identityKey: string) => {
    if (c.version !== 1 || c.deviceId !== deviceId || c.identityKey !== identityKey || c.accountKeyId !== a.accountKeyId) return false;
    const key = await crypto.subtle.importKey('spki', raw(a.accountPublicKey), { name: 'ECDSA', namedCurve: 'P-256' }, false, ['verify']);
    const input = `pockymoe-acct-device-v1\n${c.accountKeyId}\n${c.deviceId}\n${c.identityKey}\n${c.createdAt}`;
    return crypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-256' }, key, raw(c.signature), new TextEncoder().encode(input));
  });
  state.fetch.mockReset().mockImplementation(async (url: string) => {
    if (url.endsWith('/owner-account')) return Response.json(account);
    if (url.includes('/transport/key?')) return Response.json({
      challenge: new URL(url, 'https://pockymoe.test').searchParams.get('challenge'),
      identityKey: certificate.identityKey, certificate, trust: { bound: true },
    });
    throw new Error('Unexpected relay path');
  });
});
describe('account-key recipient identity', () => {
  it('accepts both valid account SPKI hashes and rejects substituted or malformed public keys', async () => {
    expect(await trust.validatePublicAccount(account)).toEqual(account);
    expect(await trust.validatePublicAccount(vectors.otherAccount)).toEqual(vectors.otherAccount);
    for (const bad of [
      { ...account, accountPublicKey: vectors.otherAccount.accountPublicKey },
      { ...account, accountKeyId: vectors.otherAccount.accountKeyId },
      { ...account, accountPublicKey: account.accountPublicKey + '=' },
    ]) await expect(trust.validatePublicAccount(bad)).rejects.toMatchObject({ code: 'account_mismatch' });
  });
  it('verifies a recipient certificate with a TOFU owner account and pins that account only after verification', async () => {
    expect(await trust.verifySharedDevice(certificate.deviceId, certificate.identityKey, certificate)).toEqual({ accountKeyId: account.accountKeyId, identityTrust: 'tofu' });
    expect(state.store.get(`tofu-owner:${certificate.deviceId}`)).toBe(account.accountKeyId);
    expect(state.fetch).toHaveBeenCalledWith(`/relay/devices/${certificate.deviceId}/owner-account`, { credentials: 'include', cache: 'no-store' });
    state.fetch.mockResolvedValue(Response.json(vectors.otherAccount));
    await expect(trust.verifySharedDevice(certificate.deviceId, certificate.identityKey, certificate)).rejects.toMatchObject({ code: 'account_mismatch' });
  });
  it('requires the invite account hash and then a valid device certificate, with no TOFU fallback', async () => {
    await trust.pinInviteOwner(certificate.deviceId, account.accountKeyId);
    expect(await trust.verifySharedDevice(certificate.deviceId, certificate.identityKey, certificate)).toEqual({ accountKeyId: account.accountKeyId, identityTrust: 'invite' });
    expect(state.store.has(`tofu-owner:${certificate.deviceId}`)).toBe(false);
    state.fetch.mockResolvedValue(Response.json(vectors.otherAccount));
    await expect(trust.verifySharedDevice(certificate.deviceId, certificate.identityKey, certificate)).rejects.toMatchObject({ code: 'account_mismatch' });
  });
  it.each([
    vectors.negative.deviceCertWrongDevice,
    vectors.negative.deviceCertWrongIdentity,
    { ...certificate, createdAt: certificate.createdAt + 1 },
    { ...certificate, signature: vectors.negative.browserCertSignedByOtherAccount.signature },
    null,
  ])('rejects altered or missing vector certificates without persisting TOFU', async (bad) => {
    await expect(trust.verifySharedDevice(certificate.deviceId, certificate.identityKey, bad as DeviceCertificate | null)).rejects.toMatchObject({ code: 'invalid_certificate' });
    expect(state.store.size).toBe(0);
  });
  it('rejects a descriptor identity that disagrees with the signed certificate', async () => {
    await expect(trust.verifySharedDevice(certificate.deviceId, vectors.negative.deviceCertWrongIdentity.identityKey, certificate)).rejects.toMatchObject({ code: 'invalid_certificate' });
  });
  it('fails closed when owner-account is missing or its id does not hash its SPKI', async () => {
    for (const response of [new Response(null, { status: 404 }), Response.json({ ...account, accountKeyId: vectors.otherAccount.accountKeyId })]) {
      state.fetch.mockResolvedValue(response);
      await expect(trust.verifySharedDevice(certificate.deviceId, certificate.identityKey, certificate)).rejects.toMatchObject({ code: 'account_mismatch' });
    }
    expect(state.store.size).toBe(0);
  });
  it('lets a confirmed invite replace an earlier TOFU owner account but rejects conflicting invite pins', async () => {
    state.store.set(`tofu-owner:${certificate.deviceId}`, vectors.otherAccount.accountKeyId);
    await trust.pinInviteOwner(certificate.deviceId, account.accountKeyId);
    expect((await trust.verifySharedDevice(certificate.deviceId, certificate.identityKey, certificate)).identityTrust).toBe('invite');
    await expect(trust.pinInviteOwner(certificate.deviceId, vectors.otherAccount.accountKeyId)).rejects.toMatchObject({ code: 'account_mismatch' });
  });
  it('forgets invite and TOFU account pins without removing a verified certificate', async () => {
    await trust.pinInviteOwner(certificate.deviceId, account.accountKeyId);
    await trust.verifySharedDevice(certificate.deviceId, certificate.identityKey, certificate);
    state.store.set(`tofu-owner:${certificate.deviceId}`, account.accountKeyId);
    await trust.forgetInviteOwner(certificate.deviceId);
    expect(await trust.inviteOwnerPin(certificate.deviceId)).toBeUndefined();
    expect(state.store.has(`tofu-owner:${certificate.deviceId}`)).toBe(false);
    expect(state.store.has(`device-certificate:${account.accountKeyId}:${certificate.deviceId}`)).toBe(true);
    expect((await trust.verifySharedDevice(certificate.deviceId, certificate.identityKey, certificate)).identityTrust).toBe('tofu');
  });
});
describe('locally certified device inventory and owner invites', () => {
  it('enumerates cryptographically valid local owner certificates even when the relay hides every device', async () => {
    state.account.mockResolvedValue({ ...account, userId: vectors.account.userId });
    await trust.rememberCertifiedDevice(certificate, account);
    state.store.set(`device-certificate:${vectors.otherAccount.accountKeyId}:other`, certificate);
    state.fetch.mockRejectedValue(new Error('Relay is offline'));
    expect(await trust.locallyCertifiedDevices()).toEqual([certificate.deviceId]);
    expect(state.fetch).not.toHaveBeenCalled();
    state.store.set(`device-certificate:${account.accountKeyId}:${certificate.deviceId}`, { ...certificate, createdAt: certificate.createdAt + 1 });
    await expect(trust.locallyCertifiedDevices()).rejects.toMatchObject({ code: 'invalid_certificate' });
  });
  it('ignores bogus invite pins for a device verified with the current owner account', async () => {
    state.account.mockResolvedValue({ ...account, userId: vectors.account.userId });
    await trust.rememberCertifiedDevice(certificate, account);
    state.store.set(`invite-owner:${certificate.deviceId}`, vectors.otherAccount.accountKeyId);
    await trust.pinInviteOwner(certificate.deviceId, 'g'.repeat(43));
    expect(await trust.inviteOwnerPin(certificate.deviceId)).toBe(vectors.otherAccount.accountKeyId);
    await trust.checkSharedInviteDevice(certificate.deviceId);
    expect(state.fetch.mock.calls.some(([url]) => url.endsWith('/owner-account'))).toBe(false);
  });
  it('never installs an invite pin over the owner vault bound-device trust root, even before the certificate is cached', async () => {
    state.account.mockResolvedValue({ ...account, userId: vectors.account.userId });
    state.bound.mockResolvedValue([certificate.deviceId]);
    await trust.pinInviteOwner(certificate.deviceId, vectors.otherAccount.accountKeyId);
    expect(await trust.inviteOwnerPin(certificate.deviceId)).toBeUndefined();
  });
  it('copies an owner account id only after the device certificate verifies against the cached owner account', async () => {
    state.account.mockResolvedValue({ ...account, userId: vectors.account.userId });
    expect(await trust.sharingOwnerAccountKeyId(certificate.deviceId)).toBe(account.accountKeyId);
    expect(await trust.locallyCertifiedDevices()).toEqual([certificate.deviceId]);
    state.account.mockResolvedValue({ ...vectors.otherAccount, userId: 'recipient' });
    await expect(trust.sharingOwnerAccountKeyId(certificate.deviceId)).rejects.toMatchObject({ code: 'invalid_certificate' });
  });
  it('never offers owner invites from TOFU, recipient invite trust or an unprotected device', async () => {
    for (const pinned of [false, true]) {
      if (pinned) await trust.pinInviteOwner(certificate.deviceId, account.accountKeyId);
      await trust.verifySharedDevice(certificate.deviceId, certificate.identityKey, certificate);
      await expect(trust.sharingOwnerAccountKeyId(certificate.deviceId)).rejects.toMatchObject({ code: 'invalid_certificate' });
    }
    state.fetch.mockResolvedValue(new Response(null, { status: 404 }));
    expect(await trust.isProtectedSharingDevice('legacy')).toBe(false);
    await expect(trust.sharingOwnerAccountKeyId('legacy')).rejects.toMatchObject({ code: 'invalid_certificate' });
  });
  it('refuses missing certificates for vault-bound or descriptor-bound devices instead of legacy fallback', async () => {
    state.account.mockResolvedValue({ ...account, userId: vectors.account.userId });
    state.bound.mockResolvedValue([certificate.deviceId]);
    state.fetch.mockResolvedValue(new Response(null, { status: 404 }));
    await expect(trust.isProtectedSharingDevice(certificate.deviceId)).rejects.toMatchObject({ code: 'invalid_certificate' });
    state.bound.mockResolvedValue([]);
    state.fetch.mockImplementation(async (url: string) => Response.json({ challenge: new URL(url, 'https://pockymoe.test').searchParams.get('challenge'), identityKey: certificate.identityKey, certificate: null, trust: { bound: true } }));
    await expect(trust.isProtectedSharingDevice(certificate.deviceId)).rejects.toMatchObject({ code: 'invalid_certificate' });
  });
  it('checks the scoped invite descriptor and owner account before destination browser registration', async () => {
    await trust.pinInviteOwner(certificate.deviceId, account.accountKeyId);
    await trust.checkSharedInviteDevice(certificate.deviceId, 'thread /?');
    expect(state.fetch.mock.calls[0]![0]).toContain('/api/threads/thread%20%2F%3F/transport/key?challenge=');
    expect(state.verify).toHaveBeenCalledWith(certificate, expect.objectContaining(account), certificate.deviceId, certificate.identityKey);
    expect(state.fetch.mock.calls.some(([url]) => url.includes('/transport/browser'))).toBe(false);
  });
  it('refuses a descriptor replay with the wrong challenge before navigation', async () => {
    state.fetch.mockResolvedValue(Response.json({ challenge: 'wrong', identityKey: certificate.identityKey, certificate }));
    await expect(trust.checkSharedInviteDevice(certificate.deviceId)).rejects.toMatchObject({ code: 'invalid_certificate' });
  });
});
