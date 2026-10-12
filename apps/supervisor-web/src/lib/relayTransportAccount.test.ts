// @vitest-environment node
import { webcrypto } from 'node:crypto';
import { beforeEach, expect, it, vi } from 'vitest';
import { Aes256Gcm, CipherSuite, DhkemP256HkdfSha256, HkdfSha256 } from '@hpke/core';
import vectors from './fixtures/accountKeyVectors.json';
import * as c from './accountCrypto';
import type { BrowserIdentity, DeviceCertificate } from './accountVault';
const state = vi.hoisted(() => ({ fetch: vi.fn(), store: new Map<string, unknown>() }));
vi.mock('./accountStore', () => ({ accountStore: async (key: string, value?: unknown) => {
  if (value === null) state.store.delete(key); else if (value !== undefined) state.store.set(key, structuredClone(value));
  return structuredClone(state.store.get(key) ?? null);
} }));
vi.mock('./sharingTrustStore', () => ({
  sharingTrustStore: async (key: string, ...write: unknown[]) => {
    const storedKey = 'sharing:' + key;
    if (write.length) { if (write[0] === null) state.store.delete(storedKey); else state.store.set(storedKey, structuredClone(write[0])); }
    return structuredClone(state.store.get(storedKey));
  },
  sharingTrustEntries: async (prefix: string) => [...state.store.entries()]
    .filter(([key]) => key.startsWith('sharing:' + prefix)).map(([key, value]) => [key.slice('sharing:'.length), structuredClone(value)]),
}));
vi.stubGlobal('crypto', webcrypto); vi.stubGlobal('fetch', state.fetch);
vi.stubGlobal('navigator', { userAgent: 'test', language: 'en-US' });
vi.stubGlobal('indexedDB', { open: () => {
  const req: any = { result: { close() {}, transaction: () => {
    const tx: any = { objectStore: () => ({ get: () => { const r: any = {}; queueMicrotask(() => r.onsuccess?.()); return r; }, getAll: () => { const r: any = { result: [] }; queueMicrotask(() => r.onsuccess?.()); return r; }, openCursor: () => { const r: any = {}; queueMicrotask(() => r.onsuccess?.()); return r; }, put() {}, delete() {} }) };
    setTimeout(() => tx.oncomplete?.(), 0); return tx;
  } } }; queueMicrotask(() => req.onsuccess?.()); return req;
} });
const transport = await import('./relayTransportCrypto');
const { relayWorkerFetch } = await import('./relayServiceWorkerFetch');
const sharedTrust = await import('./sharedDeviceTrust');
const vault = await import('./accountVaultState');
const browserTransport = await import('./accountTransport');
const suite = new CipherSuite({ kem: new DhkemP256HkdfSha256(), kdf: new HkdfSha256(), aead: new Aes256Gcm() });
const recipient = await suite.kem.generateKeyPair();
const identity = await c.importPrivate(vectors.deviceCertificate.identityPrivateKeyPkcs8);
const device = vectors.deviceCertificate.certificate.deviceId;
const endpoint = (path: string) => `https://pockymoe.test/relay/devices/${device}/api/${path}`;
let missingCertificate: boolean, registrationRevoked: boolean, applicationRevoked: boolean, rejectCount: number, grantee: boolean;
let wires: { path: string; headers: Headers; aad: string; sealed: Uint8Array; body: unknown }[];
let browser: BrowserIdentity;
let foreignCertificate: DeviceCertificate | undefined;
function packet(body: unknown, status: number) {
  const prefix = c.utf8(JSON.stringify({ status, headers: { 'content-type': 'application/json' } })), content = c.utf8(JSON.stringify(body));
  const clear = new Uint8Array(4 + prefix.length + content.length); new DataView(clear.buffer).setUint32(0, prefix.length); clear.set(prefix, 4); clear.set(content, 4 + prefix.length); return clear;
}
beforeEach(async () => {
  vi.restoreAllMocks(); foreignCertificate = undefined; transport.setTransportReporter(() => {});
  state.store.clear(); transport.clearAllTransportKeys(); vault.closeUnlockWindow(); state.fetch.mockReset();
  wires = []; missingCertificate = registrationRevoked = applicationRevoked = grantee = false; rejectCount = 0;
  const a = vectors.account;
  const pair = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']);
  const publicKey = c.encode64(await crypto.subtle.exportKey('spki', pair.publicKey));
  const fields = { accountKeyId: a.accountKeyId, browserId: await c.hash64(c.decode64(publicKey)), publicKey, createdAt: Date.now() };
  browser = { ...a, browserId: fields.browserId, privateKey: pair.privateKey,
    certificate: { version: 1, ...fields, signature: await c.sign(await c.importPrivate(a.accountPrivateKeyPkcs8), c.inputs.browser(fields)) } };
  state.store.set(`device-certificate:${a.accountKeyId}:${device}`, vectors.deviceCertificate.certificate);
  state.store.set('active', a.userId); state.store.set(`cache:${a.userId}`, { ...a, revision: 3, boundDevices: [device] }); state.store.set(`browser:${a.userId}`, browser);
  state.fetch.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
    const target = new URL(input instanceof Request ? input.url : String(input), 'https://pockymoe.test');
    if (target.pathname.endsWith('/account/browsers')) return Response.json({ browsers: [], revocations: [] });
    if (target.pathname.endsWith('/transport/key')) {
      const descriptor = { version: 1, challenge: target.searchParams.get('challenge'), keyId: 'key-1', publicKey: c.encode64(await suite.kem.serializePublicKey(recipient.publicKey)), identityKey: vectors.deviceCertificate.certificate.identityKey,
        expiresAt: Date.now() + 600000, serverTime: Date.now(), signature: '', certificate: missingCertificate ? null : foreignCertificate ?? vectors.deviceCertificate.certificate, trust: { bound: true, strict: true, accountKeyId: a.accountKeyId } };
      descriptor.signature = await c.sign(identity, `rcd-key-v1\n${descriptor.challenge}\n${descriptor.keyId}\n${descriptor.expiresAt}\n${descriptor.serverTime}\n${descriptor.publicKey}`);
      return Response.json(descriptor);
    }
    const headers = new Headers(init?.headers), method = init?.method ?? 'GET', path = target.pathname.replace(`/relay/devices/${device}`, '');
    const aad = `rcd-http-v1\n${headers.get('x-rcd-key')}\n${headers.get('x-rcd-request')}\n${method}\n${path}\n${headers.get('x-rcd-resource') ?? ''}`;
    const sealed = method === 'GET' ? c.decode64(headers.get('x-rcd-sealed')!) : new Uint8Array(init?.body as ArrayBuffer);
    const receiver = await suite.createRecipientContext({ recipientKey: recipient.privateKey, enc: c.bytes(c.decode64(headers.get('x-rcd-enc')!)), info: c.utf8('remote-codex/relay/v1') });
    const opened = new Uint8Array(await receiver.open(c.bytes(sealed), c.utf8(aad)));
    const offset = new DataView(opened.buffer).getUint32(0) + 4;
    const body = opened.length > offset ? JSON.parse(new TextDecoder().decode(opened.subarray(offset))) : null;
    wires.push({ path, headers, aad, sealed, body });
    const registering = path.endsWith('/browser'), revoked = registering ? registrationRevoked : applicationRevoked;
    const reject = !registering && !path.endsWith('/revocations') && rejectCount-- > 0;
    const response = revoked ? { code: 'browser_revoked' } : reject ? { code: 'browser_registration_required' } : registering ? { browserId: browser.browserId, role: grantee ? 'grantee' : 'owner' } : { ok: true };
    const key = await crypto.subtle.importKey('raw', await receiver.export(c.utf8('remote-codex/http-response/v1'), 32), 'AES-GCM', false, ['encrypt']);
    const encrypted = await crypto.subtle.encrypt({ name: 'AES-GCM', iv: new Uint8Array(12), additionalData: c.utf8(`${aad}\nresponse`) }, key, c.bytes(packet(response, reject || revoked ? 401 : 200)));
    return new Response(encrypted, { headers: { 'x-rcd-encrypted': '1' } });
  });
});
async function verifyWires() {
  for (const wire of wires) {
    if (wire.path.endsWith('/browser') || wire.path.endsWith('/revocations')) { expect(wire.headers.has('x-rcd-session')).toBe(false); continue; }
    expect(wire.headers.get('x-rcd-session')).toBe(browser.browserId);
    expect(await c.verify(browser.certificate.publicKey, wire.headers.get('x-rcd-signature')!, await c.requestSigningInput(wire.aad, wire.sealed))).toBe(true);
  }
}
it('syncs signed revocations and registers once before signing encrypted requests', async () => {
  expect((await transport.exchange(new Request(endpoint('version')))).response.ok).toBe(true);
  expect((await transport.exchange(new Request(endpoint('workspaces'), { method: 'POST', body: '{}' }))).response.ok).toBe(true);
  expect(wires.map(w => w.path)).toEqual(['/api/transport/revocations', '/api/transport/browser', '/api/version', '/api/workspaces']);
  expect(wires[1]!.body).toEqual(browser.certificate); await verifyWires();
});
it('service-worker fetch reads the same non-extractable key and signs file requests', async () => {
  expect((await relayWorkerFetch(new Request(endpoint('files/content')), 'https://pockymoe.test/devices/device-1/workspaces')).ok).toBe(true); await verifyWires();
});
it('re-registers and retries at most once when a device forgets registration', async () => {
  rejectCount = 1; expect((await transport.exchange(new Request(endpoint('workspaces'), { method: 'POST', body: '{}' }))).response.ok).toBe(true);
  expect(wires.filter(w => w.path.endsWith('/browser'))).toHaveLength(2); expect(wires.filter(w => w.path === '/api/workspaces')).toHaveLength(2); await verifyWires();
});
it('never keeps or retries a revoked key rejected during registration', async () => {
  registrationRevoked = true;
  await expect(transport.exchange(new Request(endpoint('version')))).rejects.toMatchObject({ code: 'browser_revoked' });
  expect(await vault.currentBrowser()).toBeNull(); expect(browserTransport.isBrowserRegistered(browser, device)).toBe(false);
  expect(wires.filter(w => w.path === '/api/version')).toHaveLength(0);
  await expect(browserTransport.browserRequestHeaders(browser, 'aad', new Uint8Array())).rejects.toMatchObject({ code: 'browser_revoked' });
});
it('discards an application-revoked key and the worker returns an actionable 401', async () => {
  await transport.exchange(new Request(endpoint('version'))); applicationRevoked = true;
  const response = await relayWorkerFetch(new Request(endpoint('version')));
  expect(response.status).toBe(401); expect(await response.json()).toMatchObject({ code: 'browser_revoked' }); expect(await vault.currentBrowser()).toBeNull();
  expect(browserTransport.isBrowserRegistered(browser, device)).toBe(false);
});
it('refuses missing certificates for boundDevices even if the relay calls it unpaired', async () => {
  missingCertificate = true;
  await expect(transport.exchange(new Request(endpoint('version')))).rejects.toMatchObject({ code: 'accountCertificateMissing' }); expect(wires).toHaveLength(0);
  await expect(browserTransport.verifyTransportDevice(device, vectors.deviceCertificate.certificate.identityKey, null, { bound: false })).rejects.toMatchObject({ code: 'accountCertificateMissing' });
});
it('refuses certificate identity disagreement and signed device revocations', async () => {
  const cert = vectors.deviceCertificate.certificate as DeviceCertificate;
  await expect(browserTransport.verifyTransportDevice(device, vectors.negative.deviceCertWrongIdentity.identityKey, cert)).rejects.toMatchObject({ code: 'accountCertificateInvalid' });
  state.store.set(`revocations:${vectors.account.userId}`, [vectors.revocations.device]);
  await expect(browserTransport.verifyTransportDevice(device, cert.identityKey, cert)).rejects.toMatchObject({ code: 'accountDeviceRevoked' });
});
it('cannot silently turn an owner registration into a grantee role', async () => {
  grantee = true;
  await expect(transport.exchange(new Request(endpoint('version')))).rejects.toMatchObject({ code: 'accountCertificateInvalid' });
});
it('allows the MAC-checked enrollment install to register before the descriptor has a certificate', async () => {
  missingCertificate = true; state.store.delete(`device-certificate:${vectors.account.accountKeyId}:${device}`);
  state.store.set(`cache:${vectors.account.userId}`, { ...vectors.account, revision: 3, boundDevices: [] });
  expect((await transport.exchange(new Request(endpoint('transport/device-certificate'), { method: 'PUT', body: JSON.stringify(vectors.deviceCertificate.certificate) }))).response.ok).toBe(true);
  expect(wires.map(w => w.path)).toEqual(['/api/transport/browser', '/api/transport/device-certificate']); await verifyWires();
  await expect(transport.exchange(new Request(endpoint('version')))).rejects.toMatchObject({ code: 'accountCertificateMissing' });
});

it('uses confirmed account trust for grantees and registers their own browser certificate', async () => {
  const pair = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
  const accountKeyId = await c.hash64(new Uint8Array(await crypto.subtle.exportKey('spki', pair.publicKey)));
  const fields = { ...vectors.deviceCertificate.certificate, accountKeyId };
  foreignCertificate = { ...fields, version: 1, signature: await c.sign(pair.privateKey, c.inputs.device(fields)) };
  const verifier = vi.spyOn(sharedTrust, 'verifySharedDevice').mockResolvedValue({ accountKeyId, identityTrust: 'invite' });
  const reporter = vi.fn(); transport.setTransportReporter(reporter); grantee = true;
  state.store.delete(`device-certificate:${vectors.account.accountKeyId}:${device}`);
  state.store.set(`cache:${vectors.account.userId}`, { ...vectors.account, revision: 3, boundDevices: [] });
  expect((await transport.exchange(new Request(endpoint('threads/thread')))).response.ok).toBe(true);
  expect(verifier).toHaveBeenCalledWith(device, fields.identityKey, foreignCertificate);
  expect(wires.find(w => w.path.endsWith('/browser'))!.body).toEqual(browser.certificate);
  expect(reporter).toHaveBeenCalledWith(expect.objectContaining({ role: 'grantee', certificateVerified: true, identityTrust: 'invite' }));
  await verifyWires();
  await expect(transport.sharingFingerprint(device, 'thread')).rejects.toMatchObject({ code: 'accountCertificateInvalid' });
  missingCertificate = true; transport.clearTransportKeyCache(device);
  await expect(transport.exchange(new Request(endpoint('threads/thread')))).rejects.toMatchObject({ code: 'accountCertificateMissing' });
});
it('refuses an owner registration answer for a recipient certificate verified through invite trust', async () => {
  foreignCertificate = { ...vectors.deviceCertificate.certificate, accountKeyId: vectors.otherAccount.accountKeyId } as DeviceCertificate;
  state.store.delete(`device-certificate:${vectors.account.accountKeyId}:${device}`);
  state.store.set(`cache:${vectors.account.userId}`, { ...vectors.account, revision: 3, boundDevices: [] });
  // Certificate cryptography is covered by sharedDeviceTrust tests. This mocks
  // its verified boundary to exercise a malicious registration role answer.
  vi.spyOn(sharedTrust, 'verifySharedDevice').mockResolvedValue({ accountKeyId: vectors.otherAccount.accountKeyId, identityTrust: 'invite' });
  await expect(transport.exchange(new Request(endpoint('threads/thread')))).rejects.toMatchObject({ code: 'accountCertificateInvalid' });
  expect(wires.map(w => w.path)).toEqual(['/api/transport/revocations', '/api/transport/browser']);
});
it('never falls back to owner registration when recipient certificate trust fails', async () => {
  foreignCertificate = { ...vectors.deviceCertificate.certificate, accountKeyId: vectors.otherAccount.accountKeyId } as DeviceCertificate;
  state.store.delete(`device-certificate:${vectors.account.accountKeyId}:${device}`);
  state.store.set(`cache:${vectors.account.userId}`, { ...vectors.account, revision: 3, boundDevices: [] });
  vi.spyOn(sharedTrust, 'verifySharedDevice').mockRejectedValue(new vault.AccountTrustError('account_mismatch'));
  await expect(transport.exchange(new Request(endpoint('threads/thread')))).rejects.toMatchObject({ code: 'account_mismatch' });
  expect(wires).toHaveLength(0);
});
it('refuses a newly learned signed device revocation before any application request', async () => {
  // Exercise the helper separately: descriptor verification precedes the list refresh.
  state.fetch.mockReset().mockResolvedValue(Response.json({ browsers: [], revocations: [vectors.revocations.device] }));
  const send = vi.fn().mockResolvedValue(undefined);
  await expect(browserTransport.syncDeviceRevocations(device, send)).rejects.toMatchObject({ code: 'accountDeviceRevoked' });
  expect(send).toHaveBeenCalledWith([vectors.revocations.device]);
});

it('does not delete a browser key on a forged plaintext registration rejection', async () => {
  const original = state.fetch.getMockImplementation()!;
  state.fetch.mockImplementation(async (input, init) => String(input).endsWith('/transport/browser')
    ? Response.json({ code: 'browser_revoked' }, { status: 401 }) : original(input, init));
  await expect(transport.exchange(new Request(endpoint('version')))).rejects.toMatchObject({ code: 'browser_revoked' });
  expect((await vault.currentBrowser())?.browserId).toBe(browser.browserId);
  expect(await browserTransport.browserRequestHeaders(browser, 'aad', new Uint8Array())).toHaveProperty('x-rcd-session', browser.browserId);
});
it('pushes recipient browser revocations to a shared device, without foreign device tombstones', async () => {
  state.store.delete(`device-certificate:${vectors.account.accountKeyId}:${device}`);
  state.store.set(`cache:${vectors.account.userId}`, { ...vectors.account, revision: 3, boundDevices: [] });
  state.store.set(`protected-device:${vectors.account.accountKeyId}:${device}`, true);
  state.fetch.mockResolvedValueOnce(Response.json({ browsers: [], revocations: [vectors.revocations.browser, vectors.revocations.device] }));
  const send = vi.fn().mockResolvedValue(undefined);
  await browserTransport.syncDeviceRevocations(device, send);
  expect(send).toHaveBeenCalledWith([vectors.revocations.browser]);
});
it('rejects even a valid owner certificate whose device identity differs from the vault pin', async () => {
  const cert = vectors.deviceCertificate.certificate as DeviceCertificate;
  state.store.set(`cache:${vectors.account.userId}`, { ...vectors.account, revision: 3, boundDevices: [{ deviceId: device, identityKey: vectors.negative.deviceCertWrongIdentity.identityKey }] });
  await expect(browserTransport.verifyTransportDevice(device, cert.identityKey, cert)).rejects.toMatchObject({ code: 'accountDeviceIdentityChanged' });
});
it('never sends a locally owned device down the recipient trust path', async () => {
  state.store.delete(`device-certificate:${vectors.account.accountKeyId}:${device}`);
  state.store.set(`cache:${vectors.account.userId}`, { ...vectors.account, revision: 3, boundDevices: [] });
  await browserTransport.markOwnedDevices([device]);
  const cert = { ...vectors.deviceCertificate.certificate, accountKeyId: vectors.otherAccount.accountKeyId } as DeviceCertificate;
  const verifier = vi.spyOn(sharedTrust, 'verifySharedDevice').mockResolvedValue({ accountKeyId: cert.accountKeyId, identityTrust: 'tofu' });
  await expect(browserTransport.verifyTransportDevice(device, cert.identityKey, cert)).rejects.toMatchObject({ code: 'accountCertificateInvalid' });
  expect(verifier).not.toHaveBeenCalled();
});

it('allows only unsigned enrollment inspection before a reinstalled identity is confirmed', async () => {
  const cert = vectors.deviceCertificate.certificate;
  state.store.set(`cache:${vectors.account.userId}`, { ...vectors.account, revision: 3, boundDevices: [{ deviceId: device, identityKey: vectors.negative.deviceCertWrongIdentity.identityKey }] });
  expect((await transport.inspectEnrollmentDevice(device)).identityKey).toBe(cert.identityKey);
  expect((await transport.exchange(new Request(endpoint('transport/enrollment')))).response.ok).toBe(true);
  expect(wires.map(w => w.path)).toEqual(['/api/transport/enrollment']);
  expect(wires[0]!.headers.has('x-rcd-session')).toBe(false);
  await expect(transport.exchange(new Request(endpoint('version')))).rejects.toMatchObject({ code: 'accountDeviceIdentityChanged' });
  expect(wires).toHaveLength(1);
});

it('allows certificate installation after exact unlocked replacement confirmation, while ordinary reads still require the certificate', async () => {
  const cert = vectors.deviceCertificate.certificate;
  missingCertificate = true;
  state.store.set(`cache:${vectors.account.userId}`, { ...vectors.account, revision: 3, boundDevices: [{ deviceId: device, identityKey: cert.identityKey }] });
  expect((await transport.exchange(new Request(endpoint('transport/device-certificate'), { method: 'PUT', body: JSON.stringify(cert) }))).response.ok).toBe(true);
  await expect(transport.exchange(new Request(endpoint('version')))).rejects.toMatchObject({ code: 'accountCertificateMissing' });
});

it.each(['invite', 'tofu'])('enforces an existing %s owner pin even before an account loads or an offline device is refused', async (kind) => {
  state.store.delete('active');
  state.store.set(`sharing:${kind}-owner:${device}`, vectors.otherAccount.accountKeyId);
  expect(await browserTransport.hasAccountDeviceCertificate(device)).toBe(true);
  state.fetch.mockResolvedValue(new Response(null, { status: 404 }));
  await expect(transport.exchange(new Request(endpoint('threads/thread')))).rejects.toMatchObject({ code: 'accountCertificateMissing' });
  expect(wires).toHaveLength(0);
});
