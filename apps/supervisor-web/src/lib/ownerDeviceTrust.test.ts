// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from 'vitest';
import vectors from './fixtures/ownerTrustVectors.json';
const state = vi.hoisted(() => ({
  store: new Map<string, unknown>(),
  fetch: vi.fn(),
}));
vi.mock('./ownerTrustStore', () => ({
  trustStore: async (key: string, ...write: unknown[]) => {
    if (write.length) {
      if (write[0] === null) state.store.delete(key);
      else state.store.set(key, structuredClone(write[0]));
    } else return structuredClone(state.store.get(key));
  },
}));
vi.stubGlobal('fetch', state.fetch);
const { deviceCertificate, saveDeviceCertificate, appendDeviceCertificate, pinInviteIdentity, checkInviteIdentity, isRecipientDevice } =
  await import('./ownerDeviceTrust');
const certificate = {
  ...vectors.deviceCertificate.certificate,
  version: 1 as const,
};
beforeEach(() => {
  state.store.clear();
  state.fetch.mockReset();
  state.fetch.mockImplementation(async () => Response.json(certificate));
});
describe('certificate identity trust', () => {
  it('requires a browser-verified credential and never trusts a relay-supplied record directly', async () => {
    await expect(deviceCertificate(certificate.deviceId)).rejects.toMatchObject(
      { code: 'owner_session_required' },
    );
    state.store.set(
      `credential:${certificate.assertion.credentialId}`,
      vectors.credentialRecord,
    );
    expect(await deviceCertificate(certificate.deviceId)).toEqual(certificate);
    state.fetch.mockResolvedValue(new Response(null, { status: 404 }));
    expect(await deviceCertificate(certificate.deviceId)).toEqual(certificate);
  });
  it('rejects a wrong device, invalid proof, or replacement identity and forbids downgrade after pairing', async () => {
    state.store.set(
      `credential:${certificate.assertion.credentialId}`,
      vectors.credentialRecord,
    );
    await saveDeviceCertificate(certificate);
    for (const change of [
      { deviceId: 'wrong-device' },
      { identityKey: 'changed' },
      { assertion: vectors.negative.wrongOrigin.assertion },
    ]) {
      state.fetch.mockImplementation(async () =>
        Response.json({ ...certificate, ...change }),
      );
      await expect(
        deviceCertificate(certificate.deviceId),
      ).rejects.toMatchObject({ code: 'ownerCertificateInvalid' });
    }
    state.fetch.mockResolvedValue(new Response(null, { status: 404 }));
    expect(await deviceCertificate(certificate.deviceId)).toEqual(certificate);
  });
  it('accepts a certificate bundle when any signer was verified, and appends without dropping existing certificates', async () => {
    const alternate = {
      ...certificate,
      assertion: { ...certificate.assertion, credentialId: 'alternate' },
    };
    state.store.set('credential:alternate', {
      ...vectors.credentialRecord,
      credentialId: 'alternate',
    });
    const bundle = {
      version: 1,
      deviceId: certificate.deviceId,
      certificates: [certificate, alternate],
    };
    state.fetch.mockImplementation(async (_url: string, init?: RequestInit) =>
      init?.method === 'PUT'
        ? new Response(null, { status: 204 })
        : Response.json(bundle),
    );
    expect(await deviceCertificate(certificate.deviceId)).toEqual(alternate);
    await appendDeviceCertificate(alternate);
    const put = state.fetch.mock.calls.find(
      (call) => call[1]?.method === 'PUT',
    );
    expect(JSON.parse(put![1].body)).toEqual(bundle);
  });
  it('offers all bundle signers for a gesture when this browser has verified none', async () => {
    const alternate = {
      ...certificate,
      assertion: { ...certificate.assertion, credentialId: 'alternate' },
    };
    state.fetch.mockResolvedValue(
      Response.json({
        version: 1,
        deviceId: certificate.deviceId,
        certificates: [certificate, alternate],
      }),
    );
    await expect(deviceCertificate(certificate.deviceId)).rejects.toMatchObject(
      {
        code: 'owner_session_required',
        credentialId: [certificate.assertion.credentialId, 'alternate'],
      },
    );
  });
  it('keeps unpaired behavior on 404, but fails closed when certificates are unavailable', async () => {
    state.fetch.mockResolvedValue(new Response(null, { status: 404 }));
    expect(await deviceCertificate(certificate.deviceId)).toBeNull();
    state.fetch.mockResolvedValue(new Response(null, { status: 503 }));
    await expect(deviceCertificate(certificate.deviceId)).rejects.toMatchObject(
      { code: 'ownerCertificateUnavailable' },
    );
  });
  it('persists invite pins, rejects conflicting invites and mismatching device identities', async () => {
    await pinInviteIdentity('device', 'fingerprint');
    expect(await checkInviteIdentity('device', 'fingerprint')).toBe(true);
    await expect(checkInviteIdentity('device', 'changed')).rejects.toMatchObject({ code: 'ownerIdentityMismatch' });
    await expect(pinInviteIdentity('device', 'changed')).rejects.toMatchObject({ code: 'ownerIdentityMismatch' });
    expect(await checkInviteIdentity('unknown', 'fingerprint')).toBe(false);
  });
  it('selects recipient access without downgrading a browser with an owner-verified certificate', async () => {
    state.fetch.mockResolvedValue(Response.json({ kind: 'shared' }));
    expect(await isRecipientDevice('device', 'thread', 'workspace')).toBe(true);
    expect(state.fetch).toHaveBeenLastCalledWith('/relay/access?deviceId=device&threadId=thread&workspaceId=workspace', expect.anything());
    state.store.set('certificate:device', certificate);
    expect(await isRecipientDevice('device')).toBe(false);
  });

});
