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
    const timer = setTimeout(() => reject(new Error('page fetch timed out')), 60_000);
    channel.port1.onmessage = (event) => {
      clearTimeout(timer);
      const reply = event.data as { status?: number; statusText?: string; headers?: [string, string][]; body?: ArrayBuffer; error?: string };
      if (reply.error || !reply.status) { reject(new Error(reply.error ?? 'page fetch failed')); return; }
      resolve(new Response([101, 204, 205, 304].includes(reply.status) ? null : reply.body ?? null,
        { status: reply.status, statusText: reply.statusText ?? '', headers: reply.headers ?? [] }));
    };
    client.postMessage({ type: 'pockymoe-page-fetch', url: request.url, headers: headerList(request.headers), method: request.method, header: PAGE_FETCH_HEADER }, [channel.port2]);
  });
}
worker.addEventListener('fetch', (event) => {
  const request = event.request;
  const pageRelay = pageRelays.get(event.clientId);
  if (!matchesRelayWorkerRequest(request, worker.location.origin, pageRelay)) return;
  event.respondWith(
    (async () => {
      const client = (event.clientId ? await worker.clients.get(event.clientId) : undefined)
        ?? (pageRelay ? undefined : (await worker.clients.matchAll({ type: 'window' }))[0]);
      if (pageRelay && pageRelay !== relayOrigin(worker.location.origin)) {
        if (!client) return new Response('Relay page unavailable', { status: 503 });
        return fetchThroughPage(client, request);
      }
      return relayWorkerFetch(request, client?.url, client ? (retry) => fetchThroughPage(client, retry) : undefined);
    })(),
  );
});
