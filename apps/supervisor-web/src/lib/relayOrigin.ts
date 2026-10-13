import { appPathname, appUrl, webBase } from './webBase';
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
  const logicalPath = appPathname(target.pathname);
  // Do not retarget external URLs or local Supervisor APIs.
  if (target.origin !== webOrigin) return path;
  const local = `${appUrl(logicalPath)}${target.search}${target.hash}`;
  if (!logicalPath.startsWith('/relay/')) return local;
  const origin = relayOrigin(webOrigin, configured);
  return origin === webOrigin ? local : `${origin}${logicalPath}${target.search}${target.hash}`;
}

export function relaySocketUrl(path: string, webOrigin = globalThis.location?.origin ?? 'http://localhost', configured = import.meta.env.VITE_POCKYMOE_RELAY_ORIGIN ?? '') {
  const url = new URL(relayUrl(path, webOrigin, configured), webOrigin);
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  return url.toString();
}

// Device routes on a separate relay are rooted there; same-origin routes use
// the Web mount. Keep wire paths (/api/...) independent of that browser mount.
export function relayPathname(url: URL): string {
  return url.origin === (globalThis.location?.origin ?? 'http://localhost')
    ? appPathname(url.pathname, webBase) : url.pathname;
}

export function relayFetchInit(path: string, init: RequestInit = {}): RequestInit {
  const endpoint = new URL(relayUrl(path), globalThis.location?.origin ?? 'http://localhost');
  return endpoint.origin === relayOrigin() && endpoint.origin !== (globalThis.location?.origin ?? 'http://localhost') && endpoint.pathname.startsWith('/relay/')
    ? { ...init, credentials: 'include' } : init;
}
