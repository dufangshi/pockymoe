import { supervisorPath } from './webBase';
import { boundDevices } from './accountVault';
import { accountRevocations } from './accountVaultState';
import { verifySharedDeviceRole } from './accountTransport';
import { relayUrl } from './relayOrigin';
import { getLocale, translate, useI18n } from '@pockymoe/thread-ui/i18n';
import workerUrl from './relayServiceWorker.ts?worker&url';
import { relayWorkerRegistration } from './staticWeb';
import { readAfterOwnerUnlock } from './ownerUnlockRecovery';
import {
  exchange,
  deviceRoute,
  trustPinnedDevice,
  scopeFromPage,
  setTransportReporter,
  SocketCipher,
  type TransportStatus,
  type WireMessage,
} from './relayTransportCrypto';
const statuses = new Map<string, TransportStatus>();
export function getTransportStatus(deviceId: string) {
  return statuses.get(deviceId);
}
const probes = new Map<string, Promise<void>>();
// Verify a small encrypted response for every online device on the portal.
// Visiting a thread must not be a prerequisite for its device's lock indicator.
export function probeDeviceEncryption(deviceId: string): Promise<void> {
  const pending = probes.get(deviceId);
  if (pending) return pending;
  const task = (async () => {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 10000);
    try {
      const scope = scopeFromPage(window.location.href, deviceId);
      const recipient = await verifySharedDeviceRole(deviceId, scope);
      const path = recipient ? scope ? supervisorPath(`api/threads/${encodeURIComponent(scope)}`) : supervisorPath('api/workspaces') : supervisorPath('api/version');
      const { response } = await exchange(new Request(
        new URL(relayUrl(`relay/devices/${encodeURIComponent(deviceId)}${path}`), window.location.href),
        { credentials: 'include', signal: controller.signal },
      ), scope);
      if (!response.ok) throw new Error(translate("workbench.deviceEncryptionCheckFailed"));
    } catch {
      // report() preserves identity-change warnings, including those emitted
      // by the handshake. Unreachable devices must never be called plaintext.
      report({ deviceId, state: 'error' });
    } finally {
      clearTimeout(timeout);
    }
  })();
  probes.set(deviceId, task);
  void task.finally(() => probes.delete(deviceId));
  return task;
}
function report(status: TransportStatus) {
  if (
    ['identity-changed', 'unlock-required'].includes(statuses.get(status.deviceId)?.state ?? '') &&
    status.state === 'error'
  )
    return;
  statuses.set(status.deviceId, status);
  window.dispatchEvent(
    new CustomEvent('remote-codex-transport', { detail: status }),
  );
}
setTransportReporter(report);
if (typeof navigator !== 'undefined' && navigator.serviceWorker)
  navigator.serviceWorker.addEventListener('message', (event) => {
    if (event.data?.type === 'remote-codex-transport')
      report(event.data.status);
  });
export async function trustDeviceIdentity(deviceId: string, identityKey: string, fingerprint: string) {
  await trustPinnedDevice(deviceId, identityKey, fingerprint);
  statuses.delete(deviceId);
  navigator.serviceWorker?.controller?.postMessage({
    type: 'remote-codex-reset-transport',
    deviceId,
  });
}
let workerReady: Promise<void> | undefined;
let workerPageReset = false;
export async function ensureRelayWorker() {
  if (!navigator.serviceWorker || !window.isSecureContext)
    throw new Error(
      translate("workbench.hTTPSAndServiceWorkerSupportAreRequired"),
    );
  workerReady ??= (async () => {
    const registration = relayWorkerRegistration(workerUrl, import.meta.env.VITE_POCKYMOE_STATIC_WEB === 'true');
    await navigator.serviceWorker.register(registration.url, registration.options);
    await navigator.serviceWorker.ready;
    if (!navigator.serviceWorker.controller)
      await new Promise<void>((done, fail) => {
        const timeout = setTimeout(() => {
          navigator.serviceWorker.removeEventListener(
            'controllerchange',
            changed,
          );
          fail(
            new Error(
              translate("workbench.encryptedFileRoutingCouldNotStartReload"),
            ),
          );
        }, 10000);
        function changed() {
          clearTimeout(timeout);
          navigator.serviceWorker.removeEventListener(
            'controllerchange',
            changed,
          );
          done();
        }
        navigator.serviceWorker.addEventListener('controllerchange', changed);
      });
  })();
  try {
    await workerReady;
    if (!workerPageReset) { navigator.serviceWorker.controller?.postMessage({ type: 'pockymoe-account-reset' }); workerPageReset = true; }
    navigator.serviceWorker.controller?.postMessage({ type: 'remote-codex-locale', locale: getLocale() });
    return await navigator.serviceWorker.ready;
  } catch (e) {
    workerReady = undefined;
    throw e;
  }
}
export async function encryptedBrowserFetch(url: string, init: RequestInit, threadScope?: string) {
  const target = new URL(relayUrl(url), window.location.href);
  const deviceId = deviceRoute(target)?.deviceId;
  const read = () => exchange(
      new Request(target, init),
      threadScope ?? scopeFromPage(window.location.href, deviceId),
      async () => { await ensureRelayWorker(); },
    );
  const method = (init.method ?? 'GET').toUpperCase();
  return (await (deviceId && ['GET', 'HEAD'].includes(method)
    ? readAfterOwnerUnlock(deviceId, read, init.signal ?? undefined)
    : read())).response;
}

// Preserve the WebSocket-shaped interface used by the thread and terminal UI.
// Setup is asynchronous; callers still get CONNECTING immediately and open/close events.
class EncryptedRelaySocket extends EventTarget {
  readyState: number = WebSocket.CONNECTING;
  readonly url: string;
  private socket?: WebSocket;
  private cipher?: SocketCipher;
  private incoming = Promise.resolve();
  private outgoing = Promise.resolve();
  private setupAbort = new AbortController();
  constructor(url: string) {
    super();
    this.url = url;
    void this.connect();
  }
  private async connect() {
    try {
      const url = new URL(this.url),
        deviceId = url.pathname.match(/\/devices\/([^/]+)\/ws$/)?.[1];
      if (!deviceId) throw new Error(translate("workbench.selectADeviceBeforeConnecting"));
      const threadId = url.searchParams.get('threadId');
      const prefix = `relay/devices/${deviceId}/api${threadId ? `/threads/${encodeURIComponent(threadId)}` : ''}`;
      const request = new Request(
        new URL(relayUrl(`${prefix}/transport/session`), window.location.href),
        {
          method: 'POST',
          credentials: 'include',
          headers: { 'content-type': 'application/json' },
          body: '{}',
        },
      );
      // Channel creation is connection setup, not an application mutation.
      const result = await readAfterOwnerUnlock(
        decodeURIComponent(deviceId),
        () => exchange(
          request.clone(),
          threadId ?? undefined,
          async () => { await ensureRelayWorker(); },
          true,
        ),
        this.setupAbort.signal,
      );
      if (result.sendKey && result.receiveKey) {
        if (!result.response.ok)
          throw new Error(translate("workbench.encryptedSessionCouldNotStart"));
        const { channelId } = (await result.response.json()) as {
          channelId: string;
        };
        this.cipher = new SocketCipher(
          channelId,
          result.sendKey,
          result.receiveKey,
        );
        url.searchParams.set('channelId', channelId);
      } else if (
        getTransportStatus(decodeURIComponent(deviceId))?.state !== 'legacy'
      )
        throw new Error(translate("workbench.encryptedSessionCouldNotStart"));
      if (this.readyState !== WebSocket.CONNECTING) return;
      const socket = new WebSocket(url);
      this.socket = socket;
      socket.addEventListener('open', () => {
        this.readyState = WebSocket.OPEN;
        this.dispatchEvent(new Event('open'));
      });
      socket.addEventListener('close', (event) => {
        this.readyState = WebSocket.CLOSED;
        this.dispatchEvent(
          new CloseEvent('close', {
            code: event.code,
            reason: event.reason,
            wasClean: event.wasClean,
          }),
        );
      });
      socket.addEventListener('error', () =>
        this.dispatchEvent(new Event('error')),
      );
      socket.addEventListener('message', (event) => {
        this.incoming = this.incoming
          .then(async () => {
            const wire = JSON.parse(event.data as string) as WireMessage;
            const data = this.cipher
              ? JSON.stringify(await this.cipher.open(wire))
              : event.data;
            this.dispatchEvent(new MessageEvent('message', { data }));
          })
          .catch(() => this.fail(translate("workbench.eventVerificationFailed")));
      });
    } catch (error) {
      this.fail(
        error instanceof Error ? error.message : translate("workbench.encryptedConnectionFailed"),
      );
    }
  }
  send(data: string) {
    if (this.readyState !== WebSocket.OPEN)
      throw new DOMException('Socket is not open', 'InvalidStateError');
    this.outgoing = this.outgoing
      .then(async () => {
        const payload = this.cipher
          ? JSON.stringify(
              await this.cipher.seal(JSON.parse(data) as WireMessage),
            )
          : data;
        if (this.socket?.readyState === WebSocket.OPEN)
          this.socket.send(payload);
      })
      .catch(() => this.fail(translate("workbench.eventSendFailed")));
  }
  close(code = 1000, reason = '') {
    this.setupAbort.abort();
    if (this.readyState === WebSocket.CLOSED) return;
    this.readyState = WebSocket.CLOSING;
    if (this.socket) this.socket.close(code, reason);
    else {
      this.readyState = WebSocket.CLOSED;
      this.dispatchEvent(
        new CloseEvent('close', { code, reason, wasClean: true }),
      );
    }
  }
  private fail(message: string) {
    if (this.readyState === WebSocket.CLOSED) return;
    this.dispatchEvent(new Event('error'));
    this.socket?.close(4000, 'Encrypted connection failed');
    this.readyState = WebSocket.CLOSED;
    this.dispatchEvent(
      new CloseEvent('close', {
        code: 4000,
        reason: message.slice(0, 120),
        wasClean: false,
      }),
    );
  }
}
export function encryptedRelaySocket(url: string): WebSocket {
  return new EncryptedRelaySocket(url) as unknown as WebSocket;
}

window.addEventListener('pockymoe-account-logout', () => {
  statuses.clear(); navigator.serviceWorker?.controller?.postMessage({ type: 'pockymoe-account-reset' });
});
const revocationsChanged = () => {
  navigator.serviceWorker?.controller?.postMessage({ type: 'pockymoe-account-reset' });
  const invalidate = (revocations: Awaited<ReturnType<typeof accountRevocations>>) => {
    for (const r of revocations) if (r.kind === 'device') report({ deviceId: r.subjectId, state: 'identity-changed', bound: true, certificateVerified: false });
  };
  // Local signed tombstones take effect even if the relay is unavailable.
  void accountRevocations().then(async local => {
    invalidate(local);
    const revocations = await accountRevocations(true); invalidate(revocations);
    const recipients = [...statuses.values()].filter(s => s.role === 'grantee' && s.certificateVerified).map(s => s.deviceId);
    const owned = await boundDevices();
    for (const id of new Set([...owned, ...recipients])) {
      if (owned.includes(id) && revocations.some(r => r.kind === 'device' && r.subjectId === id)) continue;
      const list = owned.includes(id) ? revocations : revocations.filter(r => r.kind === 'browser');
      // Best effort now; exchange also synchronizes before the next request.
      void encryptedBrowserFetch(`relay/devices/${encodeURIComponent(id)}/api/transport/revocations`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ revocations: list }) }).catch(() => {});
    }
  }).catch(() => {});
};
window.addEventListener('pockymoe-account-revocations', revocationsChanged);
window.addEventListener('pockymoe-account-revocations-changed', revocationsChanged);
