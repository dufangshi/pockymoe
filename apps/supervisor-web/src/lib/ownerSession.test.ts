// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from 'vitest';
import vectors from './fixtures/ownerTrustVectors.json';
import { authenticatorResult } from './ownerTrustTestHelpers';
import {
  bytes,
  decode64,
  requestSigningInput,
  type OwnerCredential,
} from './ownerTrustCrypto';
const state = vi.hoisted(() => ({
  store: new Map<string, unknown>(),
  fetch: vi.fn(),
  get: vi.fn(),
}));
vi.mock('./ownerTrustStore', () => ({
  clearStoredOwnerSession: async (sessionId?: string) => {
    const current = state.store.get('session') as { sessionId: string } | undefined;
    if (!sessionId || current?.sessionId === sessionId) state.store.delete('session');
    const prefix = sessionId ? `registration:${sessionId}:` : 'registration:';
    for (const key of state.store.keys())
      if (key.startsWith(prefix)) state.store.delete(key);
  },
  trustStore: async (key: string, ...write: unknown[]) => {
    if (write.length) {
      if (write[0] === null) state.store.delete(key);
      else state.store.set(key, structuredClone(write[0]));
    } else return structuredClone(state.store.get(key));
  },
}));
vi.stubGlobal('fetch', state.fetch);
vi.stubGlobal('location', new URL('https://pockymoe.test'));
vi.stubGlobal('navigator', {
  language: 'en',
  languages: ['en'],
  credentials: { get: state.get },
});
const manager = await import('./ownerSession');
const record = vectors.credentialRecord as OwnerCredential;
beforeEach(async () => {
  state.store.clear();
  state.fetch.mockReset();
  state.get.mockReset();
  state.fetch.mockImplementation(async () =>
    Response.json({ passkeys: [record] }),
  );
  const key = await crypto.subtle.importKey(
    'jwk',
    vectors.credentialPrivateJwk,
    { name: 'ECDSA', namedCurve: 'P-256' },
    false,
    ['sign'],
  );
  state.get.mockImplementation(async (options: CredentialRequestOptions) =>
    authenticatorResult(
      record,
      new Uint8Array(options.publicKey!.challenge as ArrayBuffer),
      key,
    ),
  );
});
describe('owner session manager', () => {
  it('prepares without prompting, verifies one clicked assertion and stores a non-extractable 3-day session across contexts', async () => {
    const start = await manager.prepareOwnerUnlock();
    expect(state.get).not.toHaveBeenCalled();
    const session = await start();
    expect(state.get).toHaveBeenCalledTimes(1);
    expect(state.get.mock.calls[0]![0].publicKey).toMatchObject({
      rpId: 'pockymoe.test',
      userVerification: 'required',
      allowCredentials: [
        { type: 'public-key', id: bytes(decode64(record.credentialId)) },
      ],
    });
    expect(session.registration.expiresAt - session.registration.issuedAt).toBe(
      manager.OWNER_LIFETIME,
    );
    expect(session.privateKey.extractable).toBe(false);
    await expect(
      crypto.subtle.exportKey('jwk', session.privateKey),
    ).rejects.toThrow();
    expect(await manager.verifiedCredential(record.credentialId)).toEqual(
      record,
    );
    vi.resetModules();
    const workerManager = await import('./ownerSession');
    const fromWorker = await workerManager.currentOwnerSession();
    expect(fromWorker?.sessionId).toBe(session.sessionId);
    const input = vectors.request;
    const headers = await workerManager.ownerRequestHeaders(
      fromWorker,
      input.aad,
      decode64(input.sealed),
    );
    const publicKey = await crypto.subtle.importKey(
      'spki',
      bytes(decode64(session.registration.sessionPublicKey)),
      { name: 'ECDSA', namedCurve: 'P-256' },
      false,
      ['verify'],
    );
    expect(
      await crypto.subtle.verify(
        { name: 'ECDSA', hash: 'SHA-256' },
        publicKey,
        bytes(decode64(headers['x-rcd-signature']!)),
        bytes(await requestSigningInput(input.aad, decode64(input.sealed))),
      ),
    ).toBe(true);
  });
  it('blocks substituted records without saving trust or a session', async () => {
    const other = await crypto.subtle.generateKey(
      { name: 'ECDSA', namedCurve: 'P-256' },
      true,
      ['sign', 'verify'],
    );
    const { encode64 } = await import('./ownerTrustCrypto');
    state.fetch.mockResolvedValue(
      Response.json({
        passkeys: [
          {
            ...record,
            publicKey: encode64(
              await crypto.subtle.exportKey('spki', other.publicKey),
            ),
          },
        ],
      }),
    );
    await expect((await manager.prepareOwnerUnlock())()).rejects.toMatchObject({
      code: 'ownerVerificationFailed',
    });
    expect(await manager.currentOwnerSession()).toBeUndefined();
    expect(
      await manager.verifiedCredential(record.credentialId),
    ).toBeUndefined();
  });
  it('never lets the relay choose a different origin/RP and handles no passkeys/cancel', async () => {
    for (const change of [
      { origin: 'https://evil.test' },
      { rpId: 'evil.test' },
    ]) {
      state.fetch.mockResolvedValue(
        Response.json({ passkeys: [{ ...record, ...change }] }),
      );
      await expect(manager.prepareOwnerUnlock()).rejects.toMatchObject({
        code: 'ownerVerificationFailed',
      });
    }
    expect(state.get).not.toHaveBeenCalled();
    state.fetch.mockResolvedValue(Response.json({ passkeys: [] }));
    await expect(manager.prepareOwnerUnlock()).rejects.toMatchObject({
      code: 'ownerNoPasskey',
    });
    state.fetch.mockImplementation(async () =>
      Response.json({ passkeys: [record] }),
    );
    state.get.mockResolvedValue(null);
    await expect((await manager.prepareOwnerUnlock())()).rejects.toMatchObject({
      code: 'ownerCancelled',
    });
  });
  it('restricts the gesture to the certificate signer when the browser has not verified it yet', async () => {
    const start = await manager.prepareOwnerUnlock(record.credentialId);
    expect(state.get).not.toHaveBeenCalled();
    await start();
    expect(state.get.mock.calls[0]![0].publicKey.allowCredentials).toHaveLength(
      1,
    );
    await expect(
      manager.prepareOwnerUnlock('missing-signer'),
    ).rejects.toMatchObject({ code: 'ownerCertificateSigner' });
  });
  it('tracks per-device registration independently, expires, renews on click and clears on sign-out', async () => {
    const first = await (await manager.prepareOwnerUnlock())();
    await Promise.all(
      ['a', 'b'].map((id) => manager.markSessionRegistered(first, id)),
    );
    expect(await manager.isSessionRegistered(first, 'a')).toBe(true);
    expect(await manager.isSessionRegistered(first, 'b')).toBe(true);
    expect(await manager.isSessionRegistered(first, 'c')).toBe(false);
    const now = Date.now();
    vi.spyOn(Date, 'now').mockReturnValue(first.registration.expiresAt);
    expect(await manager.currentOwnerSession()).toBeUndefined();
    expect(
      await manager.ownerRequestHeaders(first, '', new Uint8Array()),
    ).toEqual({});
    vi.spyOn(Date, 'now').mockReturnValue(now);
    const second = await (await manager.prepareOwnerUnlock())();
    expect(second.sessionId).not.toBe(first.sessionId);
    expect(await manager.isSessionRegistered(second, 'a')).toBe(false);
    await manager.clearOwnerSession();
    expect(await manager.currentOwnerSession()).toBeUndefined();
    expect(await manager.isSessionRegistered(first, 'a')).toBe(false);
    expect(await manager.isSessionRegistered(first, 'b')).toBe(false);
    expect(await manager.verifiedCredential(record.credentialId)).toEqual(record);
    expect(
      await manager.ownerRequestHeaders(undefined, '', new Uint8Array()),
    ).toEqual({});
    vi.restoreAllMocks();
  });
  it('discards a revoked key and all its device registrations, then creates a different key only after a new click', async () => {
    const revoked = await (await manager.prepareOwnerUnlock())();
    await manager.markSessionRegistered(revoked, 'a');
    await manager.markSessionRegistered(revoked, 'b');
    await manager.clearOwnerSession(revoked.sessionId);
    expect(await manager.currentOwnerSession()).toBeUndefined();
    expect(await manager.isSessionRegistered(revoked, 'a')).toBe(false);
    expect(await manager.isSessionRegistered(revoked, 'b')).toBe(false);
    const unlock = await manager.prepareOwnerUnlock();
    expect(state.get).toHaveBeenCalledTimes(1);
    expect(await manager.currentOwnerSession()).toBeUndefined();
    const renewed = await unlock();
    expect(state.get).toHaveBeenCalledTimes(2);
    expect(renewed.sessionId).not.toBe(revoked.sessionId);
    expect(renewed.registration.sessionPublicKey).not.toBe(revoked.registration.sessionPublicKey);
  });
  it('keeps a newer unlock when a delayed response revokes the previous key', async () => {
    const old = await (await manager.prepareOwnerUnlock())();
    await manager.markSessionRegistered(old, 'a');
    const renewed = await (await manager.prepareOwnerUnlock())();
    await manager.markSessionRegistered(renewed, 'a');
    await manager.clearOwnerSession(old.sessionId);
    expect((await manager.currentOwnerSession())?.sessionId).toBe(renewed.sessionId);
    expect(await manager.isSessionRegistered(old, 'a')).toBe(false);
    expect(await manager.isSessionRegistered(renewed, 'a')).toBe(true);
  });
});
