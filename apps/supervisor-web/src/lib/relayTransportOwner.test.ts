// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  Aes256Gcm,
  CipherSuite,
  DhkemP256HkdfSha256,
  HkdfSha256,
} from '@hpke/core';
import vectors from './fixtures/ownerTrustVectors.json';
import { authenticatorResult } from './ownerTrustTestHelpers';
import {
  bytes,
  decode64,
  encode64,
  requestSigningInput,
  sessionChallenge,
  verifyAssertion,
  type OwnerCredential,
  utf8,
} from './ownerTrustCrypto';
const state = vi.hoisted(() => ({
  store: new Map<string, unknown>(),
  fetch: vi.fn(),
  certificate: vi.fn(),
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
vi.mock('./ownerDeviceTrust', () => ({ deviceCertificate: state.certificate }));
vi.stubGlobal('fetch', state.fetch);
const pins = new Map<string, unknown>();
vi.stubGlobal('indexedDB', {
  open: () => {
    const request: Record<string, any> = {};
    request.result = {
      createObjectStore() {},
      close() {},
      transaction: () => {
        const tx: Record<string, any> = {};
        tx.objectStore = () => ({
          get: (key: string) => {
            const result: Record<string, any> = {};
            queueMicrotask(() => {
              result.result = pins.get(key);
              result.onsuccess?.();
            });
            return result;
          },
          put: (value: unknown, key: string) => pins.set(key, value),
          delete: (key: string) => pins.delete(key),
        });
        setTimeout(() => tx.oncomplete?.(), 0);
        return tx;
      },
    };
    queueMicrotask(() => request.onsuccess?.());
    return request;
  },
});
const transport = await import('./relayTransportCrypto');
const { relayWorkerFetch } = await import('./relayServiceWorkerFetch');
const sessionManager = await import('./ownerSession');
const suite = new CipherSuite({
  kem: new DhkemP256HkdfSha256(),
  kdf: new HkdfSha256(),
  aead: new Aes256Gcm(),
});
const recipient = await suite.kem.generateKeyPair();
const identity = await crypto.subtle.importKey(
  'jwk',
  vectors.deviceCertificate.identityPrivateJwk,
  { name: 'ECDSA', namedCurve: 'P-256' },
  false,
  ['sign'],
);
const sessionKey = await crypto.subtle.importKey(
  'jwk',
  vectors.session.privateJwk,
  { name: 'ECDSA', namedCurve: 'P-256' },
  false,
  ['sign'],
);
const ownerKey = await crypto.subtle.importKey(
  'jwk',
  vectors.credentialPrivateJwk,
  { name: 'ECDSA', namedCurve: 'P-256' },
  false,
  ['sign'],
);
const sessionVerify = await crypto.subtle.importKey(
  'spki',
  bytes(decode64(vectors.request.sessionPublicKey)),
  { name: 'ECDSA', namedCurve: 'P-256' },
  false,
  ['verify'],
);
const device = vectors.deviceCertificate.certificate.deviceId;
const url = (path: string) =>
  `https://pockymoe.test/relay/devices/${device}/api/${path}`;
let wires: {
  path: string;
  method: string;
  aad: string;
  sealed: Uint8Array;
  headers: Headers;
}[];
let rejectRemaining = Infinity;
let ownerError: 'plain' | 'encrypted' | undefined;
let registrationRevoked = false;
let applicationRevoked = false;
function packet(body: string, status = 200, streamNext?: string) {
  const prefix = utf8(
    JSON.stringify({
      status,
      headers: { 'content-type': 'application/json' },
      streamNext,
    }),
  );
  const clear = new Uint8Array(4 + prefix.length + utf8(body).length);
  new DataView(clear.buffer).setUint32(0, prefix.length);
  clear.set(prefix, 4);
  clear.set(utf8(body), 4 + prefix.length);
  return clear;
}
beforeEach(async () => {
  state.store.clear();
  pins.clear();
  state.fetch.mockReset();
  state.certificate.mockReset();
  wires = [];
  ownerError = undefined;
  registrationRevoked = false;
  applicationRevoked = false;
  rejectRemaining = Infinity;
  state.certificate.mockResolvedValue(vectors.deviceCertificate.certificate);
  transport.clearTransportKeyCache(device);
  // Old issuedAt is intentional: first contact can happen days after unlock.
  await state.store.set('session', {
    privateKey: sessionKey,
    sessionId: vectors.session.sessionId,
    registration: {
      ...vectors.session.registration,
      issuedAt: Date.now() - 24 * 60 * 60 * 1000,
      expiresAt: Date.now() + 24 * 60 * 60 * 1000,
    },
  });
  const stored = (await sessionManager.currentOwnerSession())!;
  const proof = await authenticatorResult(
    vectors.credentialRecord as OwnerCredential,
    await sessionChallenge(stored.registration),
    ownerKey,
  );
  stored.registration.assertion = {
    credentialId: vectors.credentialRecord.credentialId,
    authenticatorData: encode64(proof.response.authenticatorData),
    clientDataJSON: encode64(proof.response.clientDataJSON),
    signature: encode64(proof.response.signature),
  };
  state.store.set('session', stored);
  state.fetch.mockImplementation(
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const target = new URL(
        input instanceof Request ? input.url : String(input),
        'https://pockymoe.test',
      );
      if (target.pathname.endsWith('/transport/key')) {
        const descriptor = {
          version: 1,
          challenge: target.searchParams.get('challenge'),
          keyId: 'key-1',
          publicKey: encode64(
            await suite.kem.serializePublicKey(recipient.publicKey),
          ),
          identityKey: vectors.deviceCertificate.certificate.identityKey,
          expiresAt: Date.now() + 600000,
          serverTime: Date.now(),
          signature: '',
        };
        descriptor.signature = encode64(
          await crypto.subtle.sign(
            { name: 'ECDSA', hash: 'SHA-256' },
            identity,
            utf8(
              `rcd-key-v1\n${descriptor.challenge}\n${descriptor.keyId}\n${descriptor.expiresAt}\n${descriptor.serverTime}\n${descriptor.publicKey}`,
            ),
          ),
        );
        return Response.json(descriptor);
      }
      const headers = new Headers(init?.headers);
      const method = init?.method ?? 'GET';
      const aad = `rcd-http-v1\n${headers.get('x-rcd-key')}\n${headers.get('x-rcd-request')}\n${method}\n${target.pathname.replace(`/relay/devices/${device}`, '')}\n${headers.get('x-rcd-resource') ?? ''}`;
      const sealed =
        method === 'GET'
          ? decode64(headers.get('x-rcd-sealed')!)
          : new Uint8Array(init?.body as ArrayBuffer);
      wires.push({ path: target.pathname, method, aad, sealed, headers });
      const rejected =
        ownerError &&
        !target.pathname.endsWith('/owner-session') &&
        rejectRemaining-- > 0;
      if (rejected && ownerError === 'plain')
        return Response.json({ code: 'device_strict_mode' }, { status: 403 });
      const receiver = await suite.createRecipientContext({
        recipientKey: recipient.privateKey,
        enc: bytes(decode64(headers.get('x-rcd-enc')!)),
        info: utf8('remote-codex/relay/v1'),
      });
      const requestClear = await receiver.open(bytes(sealed), utf8(aad));
      if (target.pathname.endsWith('/owner-session')) {
        const length = new DataView(requestClear).getUint32(0);
        const registration = JSON.parse(
          new TextDecoder().decode(new Uint8Array(requestClear, 4 + length)),
        );
        expect(
          await verifyAssertion(
            vectors.credentialRecord as OwnerCredential,
            registration.assertion,
            await sessionChallenge(registration),
          ),
        ).toBe(true);
      }
      const key = await crypto.subtle.importKey(
        'raw',
        await receiver.export(utf8('remote-codex/http-response/v1'), 32),
        'AES-GCM',
        false,
        ['encrypt'],
      );
      const revoked = target.pathname.endsWith('/owner-session') ? registrationRevoked : applicationRevoked;
      const body = revoked ? '{"code":"owner_session_revoked"}' : target.pathname.endsWith('/owner-session')
        ? JSON.stringify({
            sessionId: vectors.session.sessionId,
            expiresAt: (await sessionManager.currentOwnerSession())!
              .registration.expiresAt,
          })
        : rejected
          ? '{"code":"owner_session_required"}'
          : '{"ok":true}';
      const clear = packet(
        body,
        rejected || revoked ? 401 : 200,
        target.pathname.endsWith('/files/large')
          ? '/api/transport/stream/continuation'
          : undefined,
      );
      const encrypted = await crypto.subtle.encrypt(
        {
          name: 'AES-GCM',
          iv: new Uint8Array(12),
          additionalData: utf8(`${aad}\nresponse`),
        },
        key,
        bytes(clear),
      );
      return new Response(encrypted, { headers: { 'x-rcd-encrypted': '1' } });
    },
  );
});
async function verifyWires() {
  for (const wire of wires) {
    if (wire.path.endsWith('/owner-session')) {
      expect(wire.headers.has('x-rcd-session')).toBe(false);
      expect(wire.headers.has('x-rcd-signature')).toBe(false);
      continue;
    }
    expect(wire.headers.get('x-rcd-session')).toBe(vectors.session.sessionId);
    expect(
      await crypto.subtle.verify(
        { name: 'ECDSA', hash: 'SHA-256' },
        sessionVerify,
        bytes(decode64(wire.headers.get('x-rcd-signature')!)),
        bytes(await requestSigningInput(wire.aad, wire.sealed)),
      ),
    ).toBe(true);
  }
}
describe('signed encrypted requests in main thread and service worker', () => {
  it('registers the session once on first contact, signs sealed GET, POST and WebSocket channel requests', async () => {
    for (const request of [
      new Request(url('version')),
      new Request(url('threads/start?private=query'), {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          'x-rcd-session': 'forged',
          'x-rcd-signature': 'forged',
        },
        body: '{"workspaceId":"w-1"}',
      }),
      new Request(url('transport/session'), { method: 'POST', body: '{}' }),
    ]) {
      expect((await transport.exchange(request)).response.status).toBe(200);
    }
    expect(wires.filter((w) => w.path.endsWith('/owner-session'))).toHaveLength(
      1,
    );
    expect(wires).toHaveLength(4);
    expect(wires[2]!.aad.endsWith('{"workspaceId":"w-1"}')).toBe(true);
    await verifyWires();
  });
  it('signs downloads and HEAD via the service worker helper using the persisted key', async () => {
    expect(
      (
        await relayWorkerFetch(
          new Request(url('transport/stream/download')),
          `https://pockymoe.test/devices/${device}/threads/thread-1`,
        )
      ).status,
    ).toBe(200);
    const head = await relayWorkerFetch(
      new Request(url('files/download'), { method: 'HEAD' }),
    );
    expect(await head.text()).toBe('');
    expect(wires.at(-1)!.method).toBe('GET');
    await verifyWires();
  });
  it('signs every automatic stream continuation', async () => {
    const response = await relayWorkerFetch(new Request(url('files/large')));
    expect(await response.text()).toBe('{"ok":true}{"ok":true}');
    expect(wires.at(-1)!.path.endsWith('/transport/stream/continuation')).toBe(
      true,
    );
    await verifyWires();
  });
  it('requires a click when no session exists and does not dispatch the request', async () => {
    state.store.delete('session');
    const report = vi.fn();
    transport.setTransportReporter(report);
    const response = await relayWorkerFetch(new Request(url('version')));
    expect(response.status).toBe(401);
    expect(await response.json()).toMatchObject({
      code: 'owner_session_required',
    });
    expect(wires).toHaveLength(0);
    expect(report).toHaveBeenLastCalledWith(
      expect.objectContaining({ state: 'unlock-required', paired: true }),
    );
  });
  it.each(['plain', 'encrypted'] as const)(
    'signals unlock on %s device rejection without replaying mutations',
    async (failure) => {
      const owner = (await sessionManager.currentOwnerSession())!;
      await sessionManager.markSessionRegistered(owner, device);
      ownerError = failure;
      const report = vi.fn();
      transport.setTransportReporter(report);
      const result = await transport.exchange(
        new Request(url('threads/start'), { method: 'POST', body: '{}' }),
      );
      expect(result.response.status).toBe(failure === 'plain' ? 403 : 401);
      expect(wires).toHaveLength(failure === 'plain' ? 1 : 3);
      expect(report).toHaveBeenLastCalledWith(
        expect.objectContaining({ state: 'unlock-required' }),
      );
    },
  );
  it('recovers an unknown session with one registration and one retry before displaying an unlock prompt', async () => {
    await sessionManager.markSessionRegistered(
      (await sessionManager.currentOwnerSession())!,
      device,
    );
    ownerError = 'encrypted';
    rejectRemaining = 1;
    const report = vi.fn();
    transport.setTransportReporter(report);
    const response = await relayWorkerFetch(
      new Request(url('threads/start'), { method: 'POST', body: '{}' }),
    );
    expect(response.status).toBe(200);
    expect(wires).toHaveLength(3);
    expect(wires[1]!.path.endsWith('/owner-session')).toBe(true);
    expect(
      report.mock.calls.some((call) => call[0].state === 'unlock-required'),
    ).toBe(false);
    await verifyWires();
  });
  it('keeps TOFU for unpaired devices and omits session headers even when another device is unlocked', async () => {
    state.certificate.mockResolvedValue(null);
    const result = await transport.exchange(new Request(url('version')));
    expect(result.response.status).toBe(200);
    expect(pins.get(device)).toBe(
      vectors.deviceCertificate.certificate.identityKey,
    );
    expect(wires).toHaveLength(1);
    expect(wires[0]!.headers.has('x-rcd-session')).toBe(false);
    expect(wires[0]!.headers.has('x-rcd-signature')).toBe(false);
  });
  it.each(['main', 'worker'] as const)('discards a tombstoned registration in the %s path and never dispatches the application request', async (path) => {
    const owner = (await sessionManager.currentOwnerSession())!;
    await sessionManager.markSessionRegistered(owner, 'other-device');
    registrationRevoked = true;
    const report = vi.fn();
    transport.setTransportReporter(report);
    if (path === 'main') {
      await expect(transport.exchange(new Request(url('version')))).rejects.toMatchObject({ code: 'owner_session_revoked' });
    } else {
      const response = await relayWorkerFetch(new Request(url('version')));
      expect(response.status).toBe(401);
      expect(await response.json()).toMatchObject({ code: 'owner_session_revoked' });
    }
    expect(wires).toHaveLength(1);
    expect(wires[0]!.path.endsWith('/owner-session')).toBe(true);
    expect(await sessionManager.currentOwnerSession()).toBeUndefined();
    expect(await sessionManager.isSessionRegistered(owner, 'other-device')).toBe(false);
    expect(report).toHaveBeenLastCalledWith(expect.objectContaining({
      state: 'unlock-required', unlockReason: 'owner_session_revoked',
    }));
    const locked = await relayWorkerFetch(new Request(url('version')));
    expect(locked.status).toBe(401);
    expect(wires).toHaveLength(1);
  });
  it('stops unknown-session recovery when registration is revoked, clears all marks and never retries the mutation', async () => {
    const owner = (await sessionManager.currentOwnerSession())!;
    await sessionManager.markSessionRegistered(owner, device);
    await sessionManager.markSessionRegistered(owner, 'other-device');
    ownerError = 'encrypted';
    registrationRevoked = true;
    const report = vi.fn();
    transport.setTransportReporter(report);
    const response = await relayWorkerFetch(new Request(url('threads/start'), { method: 'POST', body: '{}' }));
    expect(response.status).toBe(401);
    expect(await response.json()).toMatchObject({ code: 'owner_session_revoked' });
    expect(wires).toHaveLength(2);
    expect(wires[1]!.path.endsWith('/owner-session')).toBe(true);
    expect(await sessionManager.currentOwnerSession()).toBeUndefined();
    expect(await sessionManager.isSessionRegistered(owner, device)).toBe(false);
    expect(await sessionManager.isSessionRegistered(owner, 'other-device')).toBe(false);
    expect(report).toHaveBeenLastCalledWith(expect.objectContaining({ unlockReason: 'owner_session_revoked' }));
  });
  it('also clears a session when a signed request receives an encrypted revocation, without retrying it', async () => {
    const owner = (await sessionManager.currentOwnerSession())!;
    await sessionManager.markSessionRegistered(owner, device);
    applicationRevoked = true;
    const response = await relayWorkerFetch(new Request(url('threads/start'), { method: 'POST', body: '{}' }));
    expect(response.status).toBe(401);
    expect(await response.json()).toMatchObject({ code: 'owner_session_revoked' });
    expect(wires).toHaveLength(1);
    expect(await sessionManager.currentOwnerSession()).toBeUndefined();
    expect(await sessionManager.isSessionRegistered(owner, device)).toBe(false);
  });
  it('does not let a plaintext relay error discard the browser key', async () => {
    const owner = (await sessionManager.currentOwnerSession())!;
    await sessionManager.markSessionRegistered(owner, device);
    const server = state.fetch.getMockImplementation()!;
    state.fetch.mockImplementation((input, init) => String(input).includes('/transport/key?')
      ? server(input, init)
      : Response.json({ code: 'owner_session_revoked' }, { status: 401 }));
    await relayWorkerFetch(new Request(url('version')));
    expect((await sessionManager.currentOwnerSession())?.sessionId).toBe(owner.sessionId);
    expect(await sessionManager.isSessionRegistered(owner, device)).toBe(true);
  });
  it('does not let an unpaired device revoke the session used for paired devices', async () => {
    state.certificate.mockResolvedValue(null);
    applicationRevoked = true;
    const owner = (await sessionManager.currentOwnerSession())!;
    const response = await relayWorkerFetch(new Request(url('version')));
    expect(response.status).toBe(401);
    expect((await sessionManager.currentOwnerSession())?.sessionId).toBe(owner.sessionId);
  });
  it('also prompts when a legacy/plaintext response reports strict mode', async () => {
    state.certificate.mockResolvedValue(null);
    state.fetch.mockImplementation(async (input: RequestInfo | URL) => {
      const target = input instanceof Request ? input.url : String(input);
      return target.includes('/transport/key?')
        ? new Response(null, { status: 404 })
        : Response.json({ code: 'device_strict_mode' }, { status: 403 });
    });
    const report = vi.fn();
    transport.setTransportReporter(report);
    expect(
      (await transport.exchange(new Request(url('version')))).response.status,
    ).toBe(403);
    expect(report).toHaveBeenLastCalledWith(
      expect.objectContaining({ state: 'unlock-required' }),
    );
  });
  it('checks each QR fingerprint even if a previous pairing inspection is cached', async () => {
    const { hash64 } = await import('./ownerTrustCrypto');
    await transport.inspectPairingDevice(
      device,
      await hash64(decode64(vectors.deviceCertificate.certificate.identityKey)),
    );
    await expect(
      transport.inspectPairingDevice(device, 'wrong'),
    ).rejects.toMatchObject({ code: 'ownerIdentityMismatch' });
    transport.finishPairingInspection(device);
  });
  it('blocks a descriptor identity that disagrees with the certificate', async () => {
    state.certificate.mockResolvedValue({
      ...vectors.deviceCertificate.certificate,
      identityKey: 'different',
    });
    await expect(
      transport.exchange(new Request(url('version'))),
    ).rejects.toMatchObject({ code: 'ownerIdentityMismatch' });
    expect(wires).toHaveLength(0);
  });
});
