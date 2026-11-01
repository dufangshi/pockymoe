/// <reference lib="webworker" />
import { installPushHandlers } from './notificationWorker';
import {
  clearTransportKeyCache,
  deviceRoute,
  exchange,
  scopeFromPage,
  setTransportReporter,
  TransportError,
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
worker.addEventListener('message', (event) => {
  if (
    event.data?.type === 'remote-codex-reset-transport' &&
    typeof event.data.deviceId === 'string'
  )
    clearTransportKeyCache(event.data.deviceId);
});
worker.addEventListener('fetch', (event) => {
  const request = event.request,
    url = new URL(request.url);
  if (
    url.origin !== worker.location.origin ||
    !deviceRoute(url) ||
    request.headers.has('x-rcd-key') ||
    url.pathname.endsWith('/transport/key')
  )
    return;
  event.respondWith(
    (async () => {
      try {
        const client = event.clientId
          ? await worker.clients.get(event.clientId)
          : null;
        return (
          await exchange(
            request,
            client && deviceRoute(url)?.deviceId ===
              decodeURIComponent(new URL(client.url).pathname.match(/\/devices\/([^/]+)/)?.[1] ?? '')
              ? scopeFromPage(client.url) : undefined,
          )
        ).response;
      } catch (error) {
        return new Response(
          JSON.stringify({
            code:
              error instanceof TransportError
                ? error.code
                : 'transport_unavailable',
            message:
              error instanceof Error
                ? error.message
                : 'Encrypted connection failed.',
          }),
          {
            status: 502,
            headers: {
              'content-type': 'application/json',
              'cache-control': 'no-store',
            },
          },
        );
      }
    })(),
  );
});
