import { listPendingEnrollments } from './pendingEnrollmentStore';
import { relayUrl } from './relayOrigin';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import {
  Aes256Gcm,
  CipherSuite,
  DhkemP256HkdfSha256,
  HkdfSha256,
} from '@hpke/core';

import { currentAccount, currentBrowser, boundDevices, type DeviceCertificate } from './accountVault';
import { AccountTrustError, browserRequestHeaders, isBrowserRegistered, markBrowserRegistered,
  forgetBrowserRegistration, handleBrowserRevoked, syncDeviceRevocations, verifyTransportDevice,
  hasAccountDeviceCertificate, resetBrowserTransport } from './accountTransport';
const encoder = new TextEncoder(),
  decoder = new TextDecoder();
const suite = new CipherSuite({
  kem: new DhkemP256HkdfSha256(),
  kdf: new HkdfSha256(),
  aead: new Aes256Gcm(),
});
const nativeFetch = globalThis.fetch.bind(globalThis);
const limit = 64 * 1024 * 1024;
export const b64 = (value: ArrayBuffer | Uint8Array) => {
  const bytes = value instanceof Uint8Array ? value : new Uint8Array(value);
  let binary = '';
  for (let start = 0; start < bytes.length; start += 8192)
    binary += String.fromCharCode(...bytes.subarray(start, start + 8192));
  return btoa(binary)
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');
};
export const unb64 = (value: string) =>
  Uint8Array.from(atob(value.replace(/-/g, '+').replace(/_/g, '/')), (c) =>
    c.charCodeAt(0),
  );
function buffer(value: Uint8Array) {
  return value.slice().buffer as ArrayBuffer;
}
export class TransportError extends Error {
  constructor(
    public code: string,
    message: string,
    public status?: number,
  ) {
    super(message);
  }
}
export interface TransportStatus {
  deviceId: string;
  state: 'encrypted' | 'legacy' | 'identity-changed' | 'error' | 'unlock-required';
  paired?: boolean;
  accountKeyId?: string | undefined;
  bound?: boolean;
  unlockReason?: 'browser_revoked';
  strict?: boolean;
  certificateVerified?: boolean;
  role?: 'owner' | 'grantee';
  identityTrust?: 'certificate' | 'invite' | 'tofu';
  fingerprint?: string;
  identityKey?: string;
}
export let reportTransport = (_status: TransportStatus) => {};
export function setTransportReporter(report: typeof reportTransport) {
  reportTransport = report;
}
export interface Descriptor {
  trust?: { bound: boolean; strict: boolean; accountKeyId: string };
  certificate?: DeviceCertificate | null;
  version: number;
  challenge: string;
  keyId: string;
  publicKey: string;
  identityKey: string;
  expiresAt: number;
  serverTime: number;
  signature: string;
}
interface Key {
  inspectionOnly?: boolean;
  paired: boolean;
  certificateVerified: boolean;
  grantee?: boolean;
  identityTrust: 'certificate' | 'invite' | 'tofu';
  accountKeyId?: string | undefined;
  bound?: boolean;
  descriptor: Descriptor;
  publicKey: CryptoKey;
  offset: number;
  fingerprint: string;
}
const keys = new Map<string, Promise<Key | null>>();
export function clearAllTransportKeys() { keys.clear(); resetBrowserTransport(); }
export function clearTransportKeyCache(deviceId: string) {
  keys.delete(deviceId);
}
async function identityStore() {
  return new Promise<IDBDatabase>((done, fail) => {
    const request = indexedDB.open('remote-codex-transport-v1', 1);
    request.onupgradeneeded = () =>
      request.result.createObjectStore('identities');
    request.onsuccess = () => done(request.result);
    request.onerror = () => fail(request.error);
  });
}
async function pinned(deviceId: string, identity?: string) {
  const db = await identityStore();
  try {
    return await new Promise<string | undefined>((done, fail) => {
      const tx = db.transaction(
          'identities',
          identity ? 'readwrite' : 'readonly',
        ),
        store = tx.objectStore('identities');
      const request = store.get(deviceId);
      let result: string | undefined;
      request.onsuccess = () => {
        result = request.result as string | undefined;
        if (identity && !result) store.put(identity, deviceId);
      };
      tx.oncomplete = () => done(result);
      tx.onerror = () => fail(tx.error);
    });
  } finally {
    db.close();
  }
}
export async function resetPinnedDevice(deviceId: string) {
  const db = await identityStore();
  try {
    await new Promise<void>((done, fail) => {
      const tx = db.transaction('identities', 'readwrite');
      tx.objectStore('identities').delete(deviceId);
      tx.oncomplete = () => done();
      tx.onerror = () => fail(tx.error);
    });
  } finally {
    db.close();
  }
  keys.delete(deviceId);
}
// Pin exactly the public key whose fingerprint the user verified. Deleting the
// old pin would trust whichever device answers next, including a different key.
export async function trustPinnedDevice(deviceId: string, identityKey: string, fingerprint: string) {
  if ((await boundDevices()).includes(deviceId) || await hasAccountDeviceCertificate(deviceId)) throw new AccountTrustError('accountIdentityMismatch');
  const actual = b64(await crypto.subtle.digest('SHA-256', buffer(unb64(identityKey))));
  if (actual !== fingerprint) throw new Error(translate("workbench.theDeviceFingerprintChangedInspectItAgain"));
  const db = await identityStore();
  try {
    await new Promise<void>((done, fail) => {
      const tx = db.transaction('identities', 'readwrite');
      tx.objectStore('identities').put(identityKey, deviceId);
      tx.oncomplete = () => done();
      tx.onerror = () => fail(tx.error);
      tx.onabort = () => fail(tx.error);
    });
  } finally {
    db.close();
  }
  keys.delete(deviceId);
}
export function deviceRoute(url: URL) {
  const match = url.pathname.match(/^\/relay\/devices\/([^/]+)(\/api\/.*)$/);
  return match
    ? { deviceId: decodeURIComponent(match[1]!), path: match[2]! + url.search }
    : null;
}
export function scopeFromPage(url: string, targetDeviceId?: string) {
  const match = new URL(url).pathname.match(/\/devices\/([^/]+)\/threads\/([^/]+)/);
  // A page's primary thread grants no scope on a different device.
  if (targetDeviceId && match && decodeURIComponent(match[1]!) !== targetDeviceId) return undefined;
  return match?.[2];
}
async function keyFor(
  deviceId: string,
  path: string,
  scope?: string,
): Promise<Key | null> {
  const cached = keys.get(deviceId);
  if (cached) {
    const key = await cached;
    if (!key && !(await boundDevices()).includes(deviceId) && !await hasAccountDeviceCertificate(deviceId)) return key;
    if (!key) keys.delete(deviceId);
    if (key && !key.inspectionOnly && key.descriptor.expiresAt > Date.now() + key.offset + 60000 &&
        key.accountKeyId === (await currentAccount())?.accountKeyId &&
        (key.certificateVerified || (!key.paired && !(await boundDevices()).includes(deviceId) && await pinned(deviceId) === key.descriptor.identityKey))) {
      if (key.certificateVerified) {
        const trust = await verifyTransportDevice(deviceId, key.descriptor.identityKey, key.descriptor.certificate, key.descriptor.trust);
        key.identityTrust = trust.identityTrust;
      }
      return key;
    }
    keys.delete(deviceId);
  }
  const task = (async () => {
    const resource = path
      .split('?')[0]!
      .match(/^\/api\/(threads|workspaces)\/([^/]+)/);
    const prefix =
      resource && !['start', 'import', 'recent'].includes(resource[2]!)
        ? `/api/${resource[1]}/${resource[2]}`
        : scope
          ? `/api/threads/${scope}`
          : '/api';
    const challenge = crypto.randomUUID();
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 10000);
    let response: Response;
    try {
      response = await nativeFetch(
        relayUrl(`/relay/devices/${encodeURIComponent(deviceId)}${prefix}/transport/key?challenge=${challenge}`),
        { credentials: 'include', cache: 'no-store', signal: controller.signal },
      );
    } catch (error) {
      if (controller.signal.aborted)
        throw new TransportError('device_unresponsive', translate("workbench.handshakeTimeout"), 504);
      throw error;
    } finally {
      clearTimeout(timeout);
    }
    const expectedBound = (await boundDevices()).includes(deviceId);
    const previous = await pinned(deviceId);
    const protectedBefore = await hasAccountDeviceCertificate(deviceId);
    if (response.status === 404 && (expectedBound || protectedBefore)) throw new AccountTrustError('accountCertificateMissing');
    if (response.status === 404 && !previous && !expectedBound && !protectedBefore) {
      reportTransport({ deviceId, state: 'legacy', bound: false });
      setTimeout(() => { if (keys.get(deviceId) === task) keys.delete(deviceId); }, 10000);
      return null;
    }
    if (!response.ok) {
      const failure = (await response.clone().json().catch(() => null)) as {
        code?: string;
        message?: string;
      } | null;
      if (
        response.status === 503 &&
        failure?.code === 'service_unavailable' &&
        failure.message === 'device is offline'
      )
        throw new TransportError(
          'device_offline',
          translate("workbench.offlineRetry"),
          response.status,
        );
      if (response.status === 504)
        throw new TransportError(
          'device_unresponsive',
          translate("workbench.noResponseRetry"),
          response.status,
        );
      throw new TransportError(
        'transport_unavailable',
        response.status === 401
          ? translate("workbench.signInForDevice")
          : response.status === 403
            ? translate("workbench.deviceAccessLost")
            : response.status === 429
              ? translate("workbench.deviceBusyRetry")
              : response.status >= 500
                ? translate("workbench.connectionUnavailableRetry")
                : translate("workbench.encryptedConnectionUnavailable"),
        response.status,
      );
    }
    const descriptor = (await response.json()) as Descriptor;
    if (
      descriptor.version !== 1 ||
      descriptor.challenge !== challenge ||
      typeof descriptor.identityKey !== 'string'
    )
      throw new TransportError(
        'transport_downgrade',
        translate("workbench.encryptionUpdateRequired"),
      );
    const identity = await crypto.subtle.importKey(
      'raw',
      buffer(unb64(descriptor.identityKey)),
      { name: 'ECDSA', namedCurve: 'P-256' },
      false,
      ['verify'],
    );
    const signed = `rcd-key-v1\n${challenge}\n${descriptor.keyId}\n${descriptor.expiresAt}\n${descriptor.serverTime}\n${descriptor.publicKey}`;
    if (
      !(await crypto.subtle.verify(
        { name: 'ECDSA', hash: 'SHA-256' },
        identity,
        buffer(unb64(descriptor.signature)),
        encoder.encode(signed),
      )) ||
      descriptor.expiresAt <= descriptor.serverTime ||
      descriptor.expiresAt > descriptor.serverTime + 3660000
    )
      throw new TransportError(
        'transport_invalid_identity',
        translate("workbench.identityUnverified"),
      );
    const fingerprint = b64(
      await crypto.subtle.digest(
        'SHA-256',
        buffer(unb64(descriptor.identityKey)),
      ),
    );
    const enrollment = ['/api/transport/enrollment', '/api/transport/browser', '/api/transport/revocations', '/api/transport/device-certificate'].includes(path.split('?')[0]!);
    const inspectionOnly = path.split('?')[0] === '/api/transport/enrollment';
    let trust;
    try { trust = await verifyTransportDevice(deviceId, descriptor.identityKey, descriptor.certificate, descriptor.trust, enrollment); }
    catch (error) {
      if (!inspectionOnly || !(error instanceof AccountTrustError) || !['accountDeviceIdentityChanged', 'accountCertificateMissing', 'accountCertificateInvalid', 'account_mismatch'].includes(error.code)) throw error;
      // The enrollment MAC authenticates this descriptor later. Inspection
      // grants no browser registration, owner signature or application access.
      trust = { verified: false, bound: false, grantee: false, identityTrust: 'tofu' as const };
    }
    const certificate = trust.verified;
    const grantee = trust.grantee;
    const stored = certificate || enrollment ? undefined : await pinned(deviceId, descriptor.identityKey);
    if (stored && stored !== descriptor.identityKey) {
      reportTransport({ deviceId, state: 'identity-changed', fingerprint, identityKey: descriptor.identityKey });
      throw new TransportError(
        'transport_identity_changed',
        translate("workbench.identityChangedRetry"),
      );
    }
    return {
      ...(inspectionOnly ? { inspectionOnly: true } : {}),
      descriptor,
      paired: trust.bound,
      bound: trust.bound,
      accountKeyId: (await currentAccount())?.accountKeyId,
      grantee,
      certificateVerified: certificate,
      identityTrust: trust.identityTrust ?? 'tofu' as const,

      fingerprint,
      offset: descriptor.serverTime - Date.now(),
      publicKey: await suite.kem.deserializePublicKey(
        buffer(unb64(descriptor.publicKey)),
      ),
    };
  })();
  keys.set(deviceId, task);
  try {
    return await task;
  } catch (e) {
    if (keys.get(deviceId) === task) keys.delete(deviceId);
    throw e;
  }
}
// Enrollment authenticates the fresh identity with its pending secret MAC.
// Inspection itself grants no trust for later application requests.
export async function inspectEnrollmentDevice(deviceId: string) {
  keys.delete(deviceId);
  const key = await keyFor(deviceId, '/api/transport/enrollment');
  if (!key) throw new AccountTrustError('accountIdentityMismatch');
  return key.descriptor;
}
async function reportOwnerRequired(response: Response, deviceId: string, encryptedBrowserId?: string, trustedIdentity = false) {
  if (response.ok) return;
  const body = await response.clone().json().catch(() => null) as { code?: string } | null;
  const revoked = response.status === 401 && body?.code === 'browser_revoked' && !!encryptedBrowserId && trustedIdentity;
  if (revoked) await handleBrowserRevoked(encryptedBrowserId!);
  if (body?.code === 'browser_registration_required' && encryptedBrowserId) {
    const browser = await currentBrowser();
    if (browser?.browserId === encryptedBrowserId) forgetBrowserRegistration(browser, deviceId);
  }
  if (revoked || body?.code === 'browser_registration_required' || body?.code === 'device_strict_mode') {
    keys.delete(deviceId);
    reportTransport({ deviceId, state: 'unlock-required', paired: true, bound: true, strict: true,
      ...(revoked ? { unlockReason: 'browser_revoked' as const } : {}) });
  }
}
// Sealed bodies cannot be compressed by the relay or the CDN. Announcing gzip
// inside the sealed metadata lets the Supervisor compress before sealing; an
// older Supervisor ignores the announcement and replies uncompressed.
const canGunzip = typeof DecompressionStream === 'function';
export async function gunzip(data: ArrayBuffer | Uint8Array, max: number) {
  const reader = new Response(data instanceof Uint8Array ? buffer(data) : data)
    .body!.pipeThrough(new DecompressionStream('gzip'))
    .getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > max) {
      await reader.cancel();
      throw new Error(translate("workbench.theEncryptedResponseIsTooLarge"));
    }
    chunks.push(value);
  }
  const out = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return out.buffer;
}
function packet(headers: Headers, body: Uint8Array, query = '') {
  const metadata: Record<string, string> = {};
  for (const name of ['content-type', 'accept', 'range', 'if-none-match']) {
    const value = headers.get(name);
    if (value) metadata[name] = value;
  }
  if (canGunzip) metadata['accept-encoding'] = 'gzip';
  const prefix = encoder.encode(JSON.stringify({ headers: metadata, query }));
  const out = new Uint8Array(4 + prefix.length + body.length);
  new DataView(out.buffer).setUint32(0, prefix.length);
  out.set(prefix, 4);
  out.set(body, 4 + prefix.length);
  return out;
}
function unpack(clear: ArrayBuffer) {
  if (clear.byteLength < 4 || clear.byteLength > limit + 65540)
    throw new Error(translate("workbench.invalidEncryptedResponseSize"));
  const length = new DataView(clear).getUint32(0);
  if (length > 65536 || length + 4 > clear.byteLength)
    throw new Error(translate("workbench.invalidEncryptedResponseMetadata"));
  const meta = JSON.parse(decoder.decode(new Uint8Array(clear, 4, length))) as {
    headers: Record<string, string>;
    status: number;
    streamNext?: string | null;
    encoding?: string;
  };
  if (meta.encoding !== undefined && meta.encoding !== 'gzip')
    throw new Error(translate("workbench.invalidEncryptedResponseMetadata"));
  return {
    headers: meta.headers,
    status: meta.status,
    streamNext: meta.streamNext,
    encoding: meta.encoding,
    body: clear.slice(4 + length),
  };
}
const aesKey = (bytes: ArrayBuffer, usages: KeyUsage[]) =>
  crypto.subtle.importKey('raw', bytes, 'AES-GCM', false, usages);
export async function exchange(
  request: Request,
  scope?: string,
  beforeEncrypt?: () => Promise<void>,
  socketKeys = false,
  retryRead = true,
  followStream = true,
  retryOwner = true,
): Promise<{
  response: Response;
  sendKey?: CryptoKey;
  receiveKey?: CryptoKey;
  continuation?: string | null;
  authenticated?: boolean;
  trustedIdentity?: boolean;
}> {
  if (request.method === 'HEAD') {
    const result = await exchange(
      new Request(request, { method: 'GET' }),
      scope,
      beforeEncrypt,
    );
    await result.response.body?.cancel();
    return {
      response: new Response(null, {
        status: result.response.status,
        headers: result.response.headers,
      }),
    };
  }
  const url = new URL(request.url),
    route = deviceRoute(url);
  if (!route || request.headers.has('x-rcd-key'))
    return { response: await nativeFetch(request) };
  let key: Key | null;
  try {
    key = await keyFor(route.deviceId, route.path, scope);
  } catch (error) {
    if (error instanceof AccountTrustError && error.code === 'accountUnlockRequired') {
      reportTransport({ deviceId: route.deviceId, state: 'unlock-required', paired: true, bound: true });
    }
    if (error instanceof TransportError && error.status)
      return {
        response: new Response(
          JSON.stringify({
            code: error.status === 401 ? 'unauthorized' : error.code,
            message: error.message,
          }),
          {
            status: error.status,
            headers: { 'content-type': 'application/json' },
          },
        ),
      };
    if ((error as { code?: string })?.code && ['accountCertificateMissing', 'accountCertificateInvalid', 'accountKeyChanged', 'accountDeviceIdentityChanged', 'accountDeviceRevoked', 'account_mismatch', 'invalid_certificate'].includes((error as { code: string }).code)) reportTransport({ deviceId: route.deviceId, state: 'identity-changed', bound: true, certificateVerified: false });
    throw error;
  }
  if (!key) {
    const response = await nativeFetch(request);
    await reportOwnerRequired(response, route.deviceId);
    return { response };
  }
  await beforeEncrypt?.();
  const owner = await currentBrowser();
  const wirePath = route.path.split('?')[0]!;
  const bootstrap = ['/api/transport/browser', '/api/transport/enrollment', '/api/transport/revocations'].includes(wirePath);
  if (key.paired && !bootstrap) {
    if (!owner) {
      reportTransport({ deviceId: route.deviceId, state: 'unlock-required', paired: true, bound: true });
      throw new AccountTrustError('accountUnlockRequired');
    }
    try { await syncDeviceRevocations(route.deviceId, async revocations => {
      const result = await exchange(new Request(new URL(`/relay/devices/${encodeURIComponent(route.deviceId)}/api/transport/revocations`, url.origin), {
        method: 'PUT', credentials: request.credentials, headers: { 'content-type': 'application/json' }, body: JSON.stringify({ revocations }),
      }));
      if (!result.response.ok) throw new AccountTrustError('accountRevocationsFailed');
    }); } catch (e) {
      if (e instanceof AccountTrustError && e.code === 'browser_revoked') reportTransport({ deviceId: route.deviceId, state: 'unlock-required', paired: true, bound: true, unlockReason: 'browser_revoked' });
      if (e instanceof AccountTrustError && e.code === 'accountDeviceRevoked') reportTransport({ deviceId: route.deviceId, state: 'identity-changed', bound: true, certificateVerified: false });
      throw e;
    }
    if (!isBrowserRegistered(owner, route.deviceId)) {
      const registered = await exchange(new Request(new URL(`/relay/devices/${encodeURIComponent(route.deviceId)}/api/transport/browser`, url.origin), {
        method: 'POST', credentials: request.credentials, signal: request.signal,
        headers: { 'content-type': 'application/json' }, body: JSON.stringify(owner.certificate),
      }));
      if (!registered.response.ok) {
        const failure = await registered.response.clone().json().catch(() => null) as { code?: string } | null;
        if (registered.authenticated && registered.trustedIdentity && registered.response.status === 401 && failure?.code === 'browser_revoked') {
          await handleBrowserRevoked(owner.browserId);
          reportTransport({ deviceId: route.deviceId, state: 'unlock-required', paired: true, bound: true, unlockReason: 'browser_revoked' });
        }
        throw new AccountTrustError(failure?.code ?? 'accountRegistrationFailed');
      }
      const registration = await registered.response.json() as { browserId: string; role: 'owner' | 'grantee' };
      if (registration.browserId !== owner.browserId || registration.role !== (key.grantee ? 'grantee' : 'owner')) throw new AccountTrustError('accountCertificateInvalid');
      key.grantee = registration.role === 'grantee';
      markBrowserRegistered(owner, route.deviceId);
    }
  }
  const requestId = `${crypto.randomUUID()}.${Math.floor(Date.now() + key.offset)}`;
  const body =
    request.method === 'GET' || request.method === 'HEAD'
      ? new Uint8Array()
      : new Uint8Array(await request.arrayBuffer());
  if (body.byteLength > limit)
    throw new Error(translate("workbench.theEncryptedRequestIsTooLarge"));
  let resource = '';
  if (
    request.headers.get('content-type')?.includes('application/json') &&
    body.length
  ) {
    try {
      const data = JSON.parse(decoder.decode(body));
      if (typeof data.workspaceId === 'string')
        resource = JSON.stringify({ workspaceId: data.workspaceId });
    } catch {
      /* original endpoint validates non-JSON */
    }
  }
  const aad = `rcd-http-v1\n${key.descriptor.keyId}\n${requestId}\n${request.method}\n${wirePath}\n${resource}`;
  const sender = await suite.createSenderContext({
    recipientPublicKey: key.publicKey,
    info: encoder.encode('remote-codex/relay/v1'),
  });
  const sealed = await sender.seal(
    buffer(packet(request.headers, body, url.search)),
    encoder.encode(aad),
  );
  const headers = new Headers(request.headers);
  headers.delete('x-rcd-session');
  headers.delete('x-rcd-signature');
  const ownerHeaders = await browserRequestHeaders(
    key.paired && !bootstrap ? owner : undefined,
    aad,
    new Uint8Array(sealed),
  );
  for (const [name, value] of Object.entries(ownerHeaders)) headers.set(name, value);
  headers.set('x-rcd-key', key.descriptor.keyId);
  headers.set('x-rcd-request', requestId);
  headers.set('x-rcd-enc', b64(sender.enc));
  if (resource) headers.set('x-rcd-resource', resource);
  headers.set('content-type', 'application/octet-stream');
  const hasBody = request.method !== 'GET' && request.method !== 'HEAD';
  if (!hasBody) headers.set('x-rcd-sealed', b64(sealed));
  const wireUrl = new URL(request.url);
  wireUrl.search = '';
  const recoverOwner = async (response: Response, authenticated = false) => {
    if (!retryOwner || bootstrap || !owner || response.status !== 401)
      return null;
    const failure = await response.clone().json().catch(() => null) as {
      code?: string;
    } | null;
    if (failure?.code !== 'browser_registration_required') return null;
    const account = await currentAccount();
    const pending = !!account && (await listPendingEnrollments(account)).some(
      enrollment => enrollment.deviceId === route.deviceId,
    );
    if (!key.paired && (!pending || !authenticated)) return null;
    // A local bind can make the cached unbound descriptor obsolete. Refresh
    // only this device; browser keys and the account unlock window stay live.
    if (pending) clearTransportKeyCache(route.deviceId);
    // The device rejects this before dispatch, so a single retry is safe even
    // for mutations. Renewal or another key still requires an explicit click.
    await forgetBrowserRegistration(owner, route.deviceId);
    return exchange(
      new Request(request.url, {
        method: request.method,
        headers: request.headers,
        credentials: request.credentials,
        signal: request.signal,
        ...(hasBody ? { body: buffer(body) } : {}),
      }),
      scope,
      beforeEncrypt,
      socketKeys,
      retryRead,
      followStream,
      false,
    );
  };
  const response = await nativeFetch(wireUrl, {
    method: request.method,
    headers,
    credentials: request.credentials,
    signal: request.signal,
    cache: 'no-store',
    ...(hasBody ? { body: sealed } : {}),
  });
  if (response.headers.get('x-rcd-encrypted') !== '1') {
    if (
      response.status === 409 &&
      (
        await response
          .clone()
          .json()
          .catch(() => ({}))
      ).code === 'transport_reconnect_required'
    ) {
      clearTransportKeyCache(route.deviceId);
      // Unknown recipient keys are rejected before decryption/dispatch. Session
      // creation must recover too, but never replay arbitrary application POSTs.
      const sessionRetry = request.method === 'POST' && wirePath.endsWith('/transport/session');
      if (retryRead && (request.method === 'GET' || sessionRetry))
        return exchange(
          sessionRetry ? new Request(request.url, {
            method: request.method, headers: request.headers,
            credentials: request.credentials, signal: request.signal,
            body: buffer(body),
          }) : request,
          scope,
          beforeEncrypt,
          socketKeys,
          false,
          followStream,
          retryOwner,
        );
    }
    if (!response.ok) {
      const recovered = await recoverOwner(response);
      if (recovered) return recovered;
      await reportOwnerRequired(response, route.deviceId);
      return { response };
    }
    throw new TransportError(
      'transport_downgrade',
      translate("workbench.anUnencryptedDeviceResponseWasRejected"),
    );
  }
  const ciphertext = await response.arrayBuffer();
  if (ciphertext.byteLength > limit + 65556)
    throw new Error(translate("workbench.theEncryptedResponseIsTooLarge"));
  const responseKey = await aesKey(
    await sender.export(encoder.encode('remote-codex/http-response/v1'), 32),
    ['decrypt'],
  );
  const opened = await crypto.subtle.decrypt(
    {
      name: 'AES-GCM',
      iv: new Uint8Array(12),
      additionalData: encoder.encode(`${aad}\nresponse`),
    },
    responseKey,
    ciphertext,
  );
  const data = unpack(opened);
  if (data.encoding === 'gzip') data.body = await gunzip(data.body, limit);
  if (!key.inspectionOnly && !bootstrap) reportTransport({
    deviceId: route.deviceId,
    state: 'encrypted',
    paired: key.paired,
    strict: key.paired,
    bound: key.paired,
    certificateVerified: key.certificateVerified,
    accountKeyId: key.descriptor.certificate?.accountKeyId,
    ...(key.identityTrust ? { identityTrust: key.identityTrust } : {}),
    role: key.grantee ? 'grantee' : 'owner',
    fingerprint: key.fingerprint,
  });
  let responseBody: BodyInit | null = [204, 205, 304].includes(data.status)
    ? null
    : data.body;
  if (data.streamNext && followStream && responseBody) {
    let next: string | null | undefined = data.streamNext;
    const cancel = new AbortController();
    const abort = () => cancel.abort();
    request.signal.addEventListener('abort', abort, { once: true });
    const prefix =
      route.path
        .split('?')[0]!
        .match(/^\/api\/(threads|workspaces)\/([0-9a-f-]{36})(?:\/|$)/)?.[0]
        ?.replace(/\/$/, '') ?? '/api';
    responseBody = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(new Uint8Array(data.body));
      },
      async pull(controller) {
        if (!next) {
          request.signal.removeEventListener('abort', abort);
          controller.close();
          return;
        }
        try {
          if (
            !next.startsWith(`${prefix}/transport/stream/`) ||
            next.includes('..')
          )
            throw new Error(translate("workbench.invalidEncryptedContinuationScope"));
          const endpoint = new URL(
            `/relay/devices/${encodeURIComponent(route.deviceId)}${next}`,
            url.origin,
          );
          const part = await exchange(
            new Request(endpoint, {
              credentials: request.credentials,
              signal: cancel.signal,
            }),
            scope,
            undefined,
            false,
            true,
            false,
          );
          if (!part.response.ok)
            throw new Error(translate("workbench.downloadExpiredStartItAgain"));
          controller.enqueue(new Uint8Array(await part.response.arrayBuffer()));
          next = part.continuation;
        } catch (error) {
          request.signal.removeEventListener('abort', abort);
          controller.error(error);
        }
      },
      cancel() {
        request.signal.removeEventListener('abort', abort);
        cancel.abort();
      },
    });
  }
  const result = new Response(responseBody, {
    status: data.status,
    headers: data.headers,
  });
  const recovered = await recoverOwner(result, true);
  if (recovered) return recovered;
  const trustedIdentity = key.certificateVerified && (key.identityTrust === 'certificate' || key.identityTrust === 'invite');
  await reportOwnerRequired(result, route.deviceId, key.paired || wirePath === '/api/transport/browser' ? owner?.browserId : undefined, trustedIdentity);
  if (!socketKeys)
    return { response: result, continuation: data.streamNext ?? null, authenticated: true, trustedIdentity };
  return {
    response: result,
    authenticated: true,
    trustedIdentity,
    sendKey: await aesKey(
      await sender.export(encoder.encode('remote-codex/ws-client/v1'), 32),
      ['encrypt'],
    ),
    receiveKey: await aesKey(
      await sender.export(encoder.encode('remote-codex/ws-server/v1'), 32),
      ['decrypt'],
    ),
  };
}
export type WireMessage = {
  type?: string;
  threadId?: string | null;
  shellId?: string | null;
  encrypted?: {
    version: number;
    channelId: string;
    sequence: number;
    body: string;
    // Set on gzipped server events; bound through the distinct AAD label.
    encoding?: 'gzip';
  };
  [key: string]: unknown;
};
function wsAad(id: string, sequence: number, message: WireMessage, label = 'rcd-ws-v1') {
  return encoder.encode(
    `${label}\n${id}\n${sequence}\n${message.type ?? ''}\n${message.threadId ?? ''}\n${message.shellId ?? ''}`,
  );
}
function nonce(sequence: number) {
  const value = new Uint8Array(12);
  new DataView(value.buffer).setBigUint64(4, BigInt(sequence));
  return value;
}
export class SocketCipher {
  private sent = 0;
  private received = -1;
  constructor(
    public readonly id: string,
    private sendKey: CryptoKey,
    private receiveKey: CryptoKey,
  ) {}
  async seal(message: WireMessage) {
    const sequence = this.sent++;
    if (!Number.isSafeInteger(sequence))
      throw new Error(translate("workbench.encryptedChannelSequenceExhausted"));
    const body = await crypto.subtle.encrypt(
      {
        name: 'AES-GCM',
        iv: nonce(sequence),
        additionalData: wsAad(this.id, sequence, message),
      },
      this.sendKey,
      encoder.encode(JSON.stringify(message)),
    );
    return {
      type: message.type,
      threadId: message.threadId ?? null,
      shellId: message.shellId ?? null,
      encrypted: { version: 1, channelId: this.id, sequence, body: b64(body) },
    };
  }
  async open(message: WireMessage) {
    const encrypted = message.encrypted;
    if (
      !encrypted ||
      encrypted.version !== 1 ||
      encrypted.channelId !== this.id ||
      !Number.isSafeInteger(encrypted.sequence) ||
      encrypted.sequence <= this.received ||
      (encrypted.encoding !== undefined && encrypted.encoding !== 'gzip')
    )
      throw new Error(translate("workbench.encryptedEventReplayOrWrongChannel"));
    const gzipped = encrypted.encoding === 'gzip';
    // Relay scope filtering may omit unrelated events; monotonic gaps are valid.
    const sealed = await crypto.subtle.decrypt(
      {
        name: 'AES-GCM',
        iv: nonce(encrypted.sequence),
        additionalData: wsAad(this.id, encrypted.sequence, message, gzipped ? 'rcd-ws-gzip-v1' : 'rcd-ws-v1'),
      },
      this.receiveKey,
      buffer(unb64(encrypted.body)),
    );
    const clear = gzipped ? await gunzip(sealed, limit) : sealed;
    const parsed = JSON.parse(decoder.decode(clear)) as WireMessage;
    if (
      decoder.decode(wsAad(this.id, encrypted.sequence, parsed)) !==
      decoder.decode(wsAad(this.id, encrypted.sequence, message))
    )
      throw new Error(translate("workbench.encryptedEventRoutingMismatch"));
    this.received = encrypted.sequence;
    return parsed;
  }
}

export async function sharingFingerprint(deviceId: string, threadId?: string) {
  const key = await keyFor(deviceId, threadId ? `/api/threads/${encodeURIComponent(threadId)}` : '/api/transport/grants', threadId);
  if (!key || !key.certificateVerified || key.identityTrust !== 'certificate' || key.grantee) throw new AccountTrustError('accountCertificateInvalid');
  return key.fingerprint;
}

if (typeof window !== 'undefined') {
  window.addEventListener('pockymoe-account-logout', () => { keys.clear(); resetBrowserTransport(); });
  window.addEventListener('pockymoe-account-revocations', clearAllTransportKeys);
  window.addEventListener('pockymoe-account-revocations-changed', clearAllTransportKeys);
}
