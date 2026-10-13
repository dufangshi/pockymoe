import { relayOrigin, relayPathname } from './relayOrigin';

export function matchesRelayWorkerRequest(request: Request, webOrigin = globalThis.location?.origin ?? 'http://localhost', configured = import.meta.env.VITE_POCKYMOE_RELAY_ORIGIN ?? '') {
  const url = new URL(request.url);
  return url.origin === relayOrigin(webOrigin, configured) &&
    /^\/relay\/devices\/[^/]+\/api\//.test(relayPathname(url)) &&
    !request.headers.has('x-rcd-key') && !url.pathname.endsWith('/transport/key');
}
