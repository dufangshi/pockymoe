import { relayOrigin, relayPathname } from './relayOrigin';

/** Marks a request the page makes on the worker's behalf; the worker lets it
    pass. Named x-rcd- so the relay's CORS allows it. */
export const PAGE_FETCH_HEADER = 'x-rcd-page-fetch';

export function matchesRelayWorkerRequest(request: Request, webOrigin = globalThis.location?.origin ?? 'http://localhost', configured = import.meta.env.VITE_POCKYMOE_RELAY_ORIGIN ?? '') {
  const url = new URL(request.url);
  return url.origin === relayOrigin(webOrigin, configured) &&
    /^\/relay\/devices\/[^/]+\/api\//.test(relayPathname(url)) &&
    !request.headers.has('x-rcd-key') && !request.headers.has(PAGE_FETCH_HEADER) &&
    !url.pathname.endsWith('/transport/key');
}
