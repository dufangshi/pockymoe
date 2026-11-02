import { appPathname, appUrl, webBase } from './webBase';
let hostOrigin: string | undefined;
let hostError: Error | undefined;
export function setHostRelayOrigin(value: string) {
  if (!value.trim()) throw new Error('The mobile shell did not report a Relay origin');
  hostOrigin = validateRelayOrigin(value); hostError = undefined;
}
export function failHostRelayOrigin(error: Error) { hostError = error; }
export function configuredRelayOrigin() {
  if (hostError) throw hostError;
  return hostOrigin ?? import.meta.env.VITE_POCKYMOE_RELAY_ORIGIN ?? '';
}
function validateRelayOrigin(configured: string): string {
  const url = new URL(configured);
  const local = ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname);
  if (!(url.protocol === 'https:' || (url.protocol === 'http:' && local)) ||
      url.username || url.password || url.pathname !== '/' || url.search || url.hash)
    throw new Error('Relay must be an HTTPS origin (HTTP is allowed for localhost)');
  return url.origin;
}
// Network addressing only. Passkey validation, pins and storage use the Web origin.
export function relayOrigin(
  webOrigin = globalThis.location?.origin ?? 'http://localhost',
  configured = configuredRelayOrigin(),
): string {
  if (!configured.trim()) return webOrigin;
  return validateRelayOrigin(configured);
}

export function relayUrl(
  path: string,
  webOrigin = globalThis.location?.origin ?? 'http://localhost',
  configured = configuredRelayOrigin(),
): string {
  const target = new URL(path, webOrigin);
  // Relative inputs are logical application paths, even when the mount is
  // itself /relay/ or /api/. Rooted/absolute inputs may already be resolved.
  const logicalPath = path.startsWith('/') || /^[a-z][a-z\d+.-]*:/i.test(path)
    ? appPathname(target.pathname) : target.pathname;
  // Do not retarget external URLs or local Supervisor APIs.
  if (target.origin !== webOrigin) return path;
  const local = `${appUrl(logicalPath)}${target.search}${target.hash}`;
  if (!logicalPath.startsWith('/relay/')) return local;
  const origin = relayOrigin(webOrigin, configured);
  return origin === webOrigin ? local : `${origin}${logicalPath}${target.search}${target.hash}`;
}

export function relaySocketUrl(path: string, webOrigin = globalThis.location?.origin ?? 'http://localhost', configured = configuredRelayOrigin()) {
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
