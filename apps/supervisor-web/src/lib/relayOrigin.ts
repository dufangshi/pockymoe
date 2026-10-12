// Network addressing only. Passkey validation, pins and storage use the Web origin.
export function relayOrigin(
  webOrigin = globalThis.location?.origin ?? 'http://localhost',
  configured = import.meta.env.VITE_POCKYMOE_RELAY_ORIGIN ?? '',
): string {
  if (!configured.trim()) return webOrigin;
  const url = new URL(configured);
  const local = ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname);
  if (!(url.protocol === 'https:' || (url.protocol === 'http:' && local)) ||
      url.username || url.password || url.pathname !== '/' || url.search || url.hash)
    throw new Error('VITE_POCKYMOE_RELAY_ORIGIN must be an HTTPS origin (HTTP is allowed for localhost)');
  return url.origin;
}

export function relayUrl(
  path: string,
  webOrigin = globalThis.location?.origin ?? 'http://localhost',
  configured = import.meta.env.VITE_POCKYMOE_RELAY_ORIGIN ?? '',
): string {
  const target = new URL(path, webOrigin);
  // Do not retarget external URLs or local Supervisor APIs.
  if (target.origin !== webOrigin || !target.pathname.startsWith('/relay/')) return path;
  const origin = relayOrigin(webOrigin, configured);
  return origin === webOrigin ? path : `${origin}${target.pathname}${target.search}${target.hash}`;
}

export function relaySocketUrl(path: string, webOrigin = globalThis.location?.origin ?? 'http://localhost', configured = import.meta.env.VITE_POCKYMOE_RELAY_ORIGIN ?? '') {
  const url = new URL(path, relayOrigin(webOrigin, configured));
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  return url.toString();
}

export function relayFetchInit(path: string, init: RequestInit = {}): RequestInit {
  const endpoint = new URL(relayUrl(path), globalThis.location?.origin ?? 'http://localhost');
  return endpoint.origin === relayOrigin() && endpoint.origin !== (globalThis.location?.origin ?? 'http://localhost') && endpoint.pathname.startsWith('/relay/')
    ? { ...init, credentials: 'include' } : init;
}
