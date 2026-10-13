import { appUrl, webBase } from './webBase';
export const STATIC_RELAY_WORKER_PATH = '/relayServiceWorker.js';

export function relayWorkerRegistration(bundledUrl: string, staticWeb: boolean) {
  return {
    url: staticWeb ? appUrl(STATIC_RELAY_WORKER_PATH) : bundledUrl,
    options: { scope: webBase, type: 'module' as const },
  };
}

// Pages cannot deliver frame-ancestors as an HTTP header. Refuse to initialize
// the trusted account UI in a frame, even when navigating the parent is blocked.
export function requireTopLevelWeb(page: Window) {
  if (page.top === page.self) return;
  page.document.documentElement.replaceChildren();
  try {
    page.top?.location.replace(page.location.href);
  } catch {
    // Cross-origin or sandboxed frames can prohibit top navigation.
  }
  throw new Error('The account Web app requires a top-level window');
}
