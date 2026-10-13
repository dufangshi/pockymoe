/// <reference lib="webworker" />
import { appPathname, appUrl } from './webBase';
import { translate } from '@pockymoe/thread-ui/i18n';
const threadPath = /^\/devices\/[0-9a-f-]{36}\/threads\/[0-9a-f-]{36}\/?$/i;
export function notificationThreadUrl(
  raw: unknown,
  origin: string,
): URL | null {
  if (typeof raw !== 'string') return null;
  try {
    const url = new URL(raw.startsWith('/') ? appUrl(appPathname(raw)) : raw, origin);
    return url.origin === origin && threadPath.test(appPathname(url.pathname)) ? url : null;
  } catch {
    return null;
  }
}
async function currentUrl(client: WindowClient): Promise<string | null> {
  const channel = new MessageChannel();
  return new Promise((resolve) => {
    const finish = (url: string | null) => {
      clearTimeout(timer);
      channel.port1.close();
      channel.port2.close();
      resolve(url);
    };
    const timer = setTimeout(() => finish(null), 500);
    channel.port1.onmessage = (event) =>
      finish(typeof event.data?.url === 'string' ? event.data.url : null);
    try {
      client.postMessage({ type: 'remote-codex-current-route' }, [
        channel.port2,
      ]);
    } catch {
      finish(null);
    }
  });
}
export async function focusNotificationThread(
  clients: Clients,
  raw: unknown,
  origin: string,
) {
  const target = notificationThreadUrl(raw, origin);
  if (!target) return;
  const windows = await clients.matchAll({
    type: 'window',
    includeUncontrolled: true,
  });
  const routes = await Promise.all(windows.map(currentUrl));
  for (let i = 0; i < windows.length; i++) {
    const route = notificationThreadUrl(routes[i], origin);
    if (
      route?.pathname.replace(/\/$/, '') === target.pathname.replace(/\/$/, '')
    ) {
      try {
        await windows[i]!.focus();
        return;
      } catch {
        /* The tab may have closed. */
      }
    }
  }
  await clients.openWindow(target.href);
}
export function installPushHandlers(worker: ServiceWorkerGlobalScope) {
  worker.addEventListener('push', (event) => {
    event.waitUntil(
      (async () => {
        let payload: Record<string, unknown>;
        try {
          payload = event.data?.json();
        } catch {
          return;
        }
        if (
          !payload ||
          !notificationThreadUrl(payload.url, worker.location.origin)
        )
          return;
        await worker.registration.showNotification('Pockymoe', {
          body:
            typeof payload.body === 'string'
              ? payload.body
              : translate('workbench.threadUpdateFallback'),
          tag: typeof payload.tag === 'string' ? payload.tag : '',
          icon: appUrl('icon-192.png'),
          badge: appUrl('favicon-48x48.png?v=pockymoe'),
          data: { url: payload.url, userId: payload.userId },
        });
      })(),
    );
  });
  worker.addEventListener('notificationclick', (event) => {
    event.notification.close();
    event.waitUntil(
      focusNotificationThread(
        worker.clients,
        event.notification.data?.url,
        worker.location.origin,
      ),
    );
  });
}
