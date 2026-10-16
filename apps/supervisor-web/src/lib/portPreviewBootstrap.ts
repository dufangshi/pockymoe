import { appPathname } from './webBase';
import { relayFetchInit, relayUrl } from './relayOrigin';
import { previewLeaseFromQuery, reusablePreviewLease } from './previewLease';

/** Redeem existing device authorization without booting the account workbench. */
export async function tryOpenPortPreview(): Promise<boolean> {
  const match = appPathname(window.location.pathname).match(/^\/devices\/([^/]+)\/ports\/([^/]+)\/?$/);
  if (!match) return false;
  const query = new URLSearchParams(window.location.search);
  const lease = previewLeaseFromQuery(query);
  if (!reusablePreviewLease(lease)) return false;
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 10_000);
  try {
    const target = relayUrl(`relay/devices/${match[1]}/port-mappings/${match[2]}/open?lease=${encodeURIComponent(lease!.leaseId)}`);
    // This is the normal, uncached owner-only POST. HttpOnly account cookies,
    // relay CSRF checks and the device's strict lease checks remain mandatory.
    const response = await fetch(target, relayFetchInit(target, {
      method: 'POST', credentials: 'include', cache: 'no-store',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: query.get('path') ?? '/' }), signal: controller.signal,
    }));
    if (!response.ok) return false;
    const { url } = await response.json() as { url?: unknown };
    if (typeof url !== 'string') return false;
    const destination = new URL(url);
    if (!['http:', 'https:'].includes(destination.protocol) || destination.username || destination.password) return false;
    window.location.replace(destination.href);
    return true;
  } catch {
    // Anonymous sessions, offline/older relays and failed handoffs retain the
    // full app's sign-in, error reporting and lease renewal behavior.
    return false;
  } finally {
    clearTimeout(timeout);
  }
}
