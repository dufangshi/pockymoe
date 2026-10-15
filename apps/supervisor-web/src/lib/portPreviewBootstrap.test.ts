import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { tryOpenPortPreview } from './portPreviewBootstrap';

const replace = vi.fn();
const fetchMock = vi.fn();
function visit(query = '?lease=cli_lease&path=%2Fapp%3Fq%3D1%23section', pathname = '/devices/device/ports/mapping') {
  vi.stubGlobal('window', { location: { pathname, search: query, origin: 'http://localhost', href: `http://localhost${pathname}${query}`, replace } });
}
beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('fetch', fetchMock);
  visit();
  fetchMock.mockResolvedValue(new Response(JSON.stringify({ url: 'https://preview.example.test/app?__rc_launch=fixture' })));
});
afterEach(() => { vi.unstubAllGlobals(); vi.unstubAllEnvs(); vi.useRealTimers(); });

it('redeems the supplied lease through the normal authenticated owner POST without loading the workbench', async () => {
  expect(await tryOpenPortPreview()).toBe(true);
  expect(fetchMock).toHaveBeenCalledOnce();
  expect(fetchMock).toHaveBeenCalledWith('/relay/devices/device/port-mappings/mapping/open?lease=cli_lease', expect.objectContaining({
    method: 'POST', credentials: 'include', cache: 'no-store',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ path: '/app?q=1#section' }), signal: expect.any(AbortSignal),
  }));
  expect(replace).toHaveBeenCalledWith('https://preview.example.test/app?__rc_launch=fixture');
});

it('sends account cookies to the configured separate relay origin', async () => {
  vi.stubEnv('VITE_POCKYMOE_RELAY_ORIGIN', 'https://relay.example.test');
  expect(await tryOpenPortPreview()).toBe(true);
  expect(fetchMock).toHaveBeenCalledWith('https://relay.example.test/relay/devices/device/port-mappings/mapping/open?lease=cli_lease', expect.objectContaining({ credentials: 'include' }));
});

it.each([
  ['', '/devices/device/ports/mapping'],
  ['?lease=expired&leaseExpiresAt=1', '/devices/device/ports/mapping'],
  ['?lease=cli_lease', '/devices/device/threads/thread'],
])('leaves missing/expired leases and other routes to the full app (%s, %s)', async (query, pathname) => {
  visit(query, pathname);
  expect(await tryOpenPortPreview()).toBe(false);
  expect(fetchMock).not.toHaveBeenCalled();
  expect(replace).not.toHaveBeenCalled();
});

it.each([401, 403, 404, 409, 503])('preserves the full sign-in/error flow after HTTP %s without redirecting', async status => {
  fetchMock.mockResolvedValue(new Response('{}', { status }));
  expect(await tryOpenPortPreview()).toBe(false);
  expect(replace).not.toHaveBeenCalled();
});

it('falls back if the relay is unreachable', async () => {
  fetchMock.mockRejectedValue(new TypeError('offline'));
  expect(await tryOpenPortPreview()).toBe(false);
  expect(replace).not.toHaveBeenCalled();
});

it.each(['javascript:alert(1)', 'https://user:secret@evil.example/'])('refuses an unsafe handoff destination %s', async url => {
  fetchMock.mockResolvedValue(new Response(JSON.stringify({ url })));
  expect(await tryOpenPortPreview()).toBe(false);
  expect(replace).not.toHaveBeenCalled();
});

it('bounds a stalled handoff before returning to the normal app', async () => {
  vi.useFakeTimers();
  fetchMock.mockImplementation((_url, init: RequestInit) => new Promise((_resolve, reject) => {
    init.signal!.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')));
  }));
  const result = tryOpenPortPreview();
  await vi.advanceTimersByTimeAsync(10_000);
  expect(await result).toBe(false);
  expect(replace).not.toHaveBeenCalled();
});
