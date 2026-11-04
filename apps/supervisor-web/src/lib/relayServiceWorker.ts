import { relayOrigin } from './relayOrigin';
import { matchesRelayWorkerRequest } from './relayWorkerMatching';
import { setLocale } from '@pockymoe/thread-ui/i18n';
/// <reference lib="webworker" />
import { relayWorkerFetch } from './relayServiceWorkerFetch';
import { PAGE_FETCH_HEADER } from './relayWorkerMatching';
import { installPushHandlers } from './notificationWorker';
import {
  clearTransportKeyCache,
  clearAllTransportKeys,
  setTransportReporter,
} from './relayTransportCrypto';
const worker = self as unknown as ServiceWorkerGlobalScope;
installPushHandlers(worker);
worker.addEventListener('install', (event) => {
  event.waitUntil(worker.skipWaiting());
});
worker.addEventListener('activate', (event) => {
  event.waitUntil(worker.clients.claim());
});
setTransportReporter((status) => {
  void worker.clients.matchAll({ type: 'window' }).then((clients) => {
    for (const client of clients)
      client.postMessage({ type: 'remote-codex-transport', status });
  });
});
const pageRelays = new Map<string, string>();
const pendingPageRelays = new Map<string, Promise<string | undefined>>();
const WORKER_HANDLED_HEADER = 'x-pockymoe-relay-worker';

function markWorkerResponse(response: Response) {
  // Legacy cross-origin no-cors reads may be opaque; their status/body cannot
  // be reconstructed. A diagnostic CORS read still receives the marker.
  if (response.type === 'opaque' || response.type === 'opaqueredirect') return response;
  const headers = new Headers(response.headers);
  headers.set(WORKER_HANDLED_HEADER, 'handled');
  return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
}

function isPotentialCrossOriginRelayRequest(request: Request) {
  const url = new URL(request.url);
  return url.origin !== worker.location.origin &&
    /^\/relay\/devices\/[^/]+\/api\//.test(url.pathname) &&
    !request.headers.has('x-rcd-key') && !request.headers.has(PAGE_FETCH_HEADER) &&
    !url.pathname.endsWith('/transport/key');
}

function askPageForRelay(clientId: string, client: Client): Promise<string | undefined> {
  const pending = pendingPageRelays.get(clientId);
  if (pending) return pending;
  const task = new Promise<string | undefined>((resolve) => {
    const channel = new MessageChannel();
    const timer = setTimeout(() => { channel.port1.close(); resolve(undefined); }, 10_000);
    channel.port1.onmessage = (event) => {
      clearTimeout(timer);
      channel.port1.close();
      try {
        const origin = typeof event.data?.origin === 'string' ? relayOrigin(worker.location.origin, event.data.origin) : undefined;
        if (origin) pageRelays.set(clientId, origin);
        resolve(origin);
      } catch { resolve(undefined); }
    };
    try { client.postMessage({ type: 'pockymoe-page-relay-request' }, [channel.port2]); }
    catch { clearTimeout(timer); channel.port1.close(); resolve(undefined); }
  });
  pendingPageRelays.set(clientId, task);
  void task.finally(() => pendingPageRelays.delete(clientId));
  return task;
}
worker.addEventListener('message', (event) => {
  if (event.data?.type === 'pockymoe-page-relay' && event.source && 'id' in event.source && typeof event.data.origin === 'string') {
    const page = event.source as Client;
    // Only a controlled page on this Web origin configures its own routing;
    // another window's Relay must never overwrite that page's encrypted channel.
    if (new URL(page.url).origin === worker.location.origin) {
      try { pageRelays.set(page.id, relayOrigin(worker.location.origin, event.data.origin)); event.ports[0]?.postMessage({ ok: true }); } catch { /* Invalid origins are not acknowledged. */ }
    }
  }
  if (event.data?.type === 'pockymoe-account-reset') clearAllTransportKeys();
  if (event.data?.type === 'remote-codex-locale' && typeof event.data.locale === 'string')
    setLocale(event.data.locale, false);
  if (
    event.data?.type === 'remote-codex-reset-transport' &&
    typeof event.data.deviceId === 'string'
  )
    clearTransportKeyCache(event.data.deviceId);
});
function headerList(headers: Headers) {
  const list: [string, string][] = [];
  headers.forEach((value, name) => list.push([name, value]));
  return list;
}
/** Asks a page to make a read for the worker (see relayWorkerFetch). */
function fetchThroughPage(client: Client, request: Request): Promise<Response> {
  return new Promise((resolve, reject) => {
    const channel = new MessageChannel();
    const timer = setTimeout(() => { channel.port1.close(); reject(new Error('page fetch timed out')); }, 15_000);
    channel.port1.onmessage = (event) => {
      clearTimeout(timer);
      channel.port1.close();
      const reply = event.data as { status?: number; statusText?: string; headers?: [string, string][]; body?: ArrayBuffer; error?: string };
      if (reply.error || !reply.status) { reject(new Error(reply.error ?? 'page fetch failed')); return; }
      resolve(new Response([101, 204, 205, 304].includes(reply.status) ? null : reply.body ?? null,
        { status: reply.status, statusText: reply.statusText ?? '', headers: reply.headers ?? [] }));
    };
    try { client.postMessage({ type: 'pockymoe-page-fetch', url: request.url, headers: headerList(request.headers), method: request.method, header: PAGE_FETCH_HEADER }, [channel.port2]); }
    catch (error) { clearTimeout(timer); channel.port1.close(); reject(error); }
  });
}
worker.addEventListener('fetch', (event) => {
  const request = event.request;
  const configuredPageRelay = pageRelays.get(event.clientId);
  if (configuredPageRelay
    ? !matchesRelayWorkerRequest(request, worker.location.origin, configuredPageRelay)
    : !isPotentialCrossOriginRelayRequest(request) && !matchesRelayWorkerRequest(request, worker.location.origin)) return;
  // respondWith must run synchronously. A cold worker claims candidate reads
  // while it asks their actual client for its Relay; it never sends them raw.
  event.respondWith((async () => {
    const client = event.clientId ? await worker.clients.get(event.clientId) : undefined;
    const ownClient = client && new URL(client.url).origin === worker.location.origin ? client : undefined;
    const pageRelay = configuredPageRelay ?? (ownClient && isPotentialCrossOriginRelayRequest(request)
      ? await askPageForRelay(event.clientId, ownClient) : undefined);
    if (!matchesRelayWorkerRequest(request, worker.location.origin, pageRelay))
      return markWorkerResponse(new Response('Relay page unavailable', { status: 503 }));
    try {
      if (pageRelay && pageRelay !== relayOrigin(worker.location.origin)) {
        if (!ownClient) return markWorkerResponse(new Response('Relay page unavailable', { status: 503 }));
        return markWorkerResponse(await fetchThroughPage(ownClient, request));
      }
      // Never borrow another window's keys or ask a hidden unrelated window.
      return markWorkerResponse(await relayWorkerFetch(request, ownClient?.url,
        ownClient ? (retry) => fetchThroughPage(ownClient, retry) : undefined));
    } catch {
      return markWorkerResponse(new Response('Relay page unavailable', { status: 502 }));
    }
  })());
});
