import { setLocale } from '@pockymoe/thread-ui/i18n';
/// <reference lib="webworker" />
import { relayWorkerFetch } from './relayServiceWorkerFetch';
import { installPushHandlers } from './notificationWorker';
import {
  clearTransportKeyCache,
  deviceRoute,
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
worker.addEventListener('message', (event) => {
  if (event.data?.type === 'remote-codex-locale' && typeof event.data.locale === 'string')
    setLocale(event.data.locale, false);
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
      const client = event.clientId ? await worker.clients.get(event.clientId) : null;
      return relayWorkerFetch(request, client?.url);
    })(),
  );
});
