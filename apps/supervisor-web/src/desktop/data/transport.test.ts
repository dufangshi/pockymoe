import { beforeEach, describe, expect, it, vi } from 'vitest';
const exchange = vi.hoisted(() => vi.fn(async () => ({ response: new Response('{}') })));
vi.mock('../../lib/relayTransportCrypto', () => ({
  exchange, trustPinnedDevice: vi.fn(), setTransportReporter: vi.fn(), SocketCipher: class {},
  scopeFromPage: (url: string) => new URL(url).pathname.match(/\/devices\/[^/]+\/threads\/([^/]+)/)?.[1],
  deviceRoute: (url: URL) => { const match = url.pathname.match(/^\/relay\/devices\/([^/]+)(\/api\/.*)$/); return match ? { deviceId: decodeURIComponent(match[1]!), path: match[2]! } : null; },
}));
import { encryptedBrowserFetch } from '../../lib/relayTransport';
beforeEach(() => { exchange.mockClear(); window.history.replaceState(null, '', '/devices/device-a/threads/thread-a'); });
describe('desktop explicit-device encrypted transport', () => {
  it('keeps each request device distinct and never borrows another device thread scope', async () => {
    await Promise.all(['device-a', 'device-b', 'device-c'].map(id => encryptedBrowserFetch(`/relay/devices/${id}/api/threads`, {})));
    expect(exchange.mock.calls.map(call => (call as unknown as [Request])[0].url)).toEqual([
      'http://localhost:3000/relay/devices/device-a/api/threads',
      'http://localhost:3000/relay/devices/device-b/api/threads',
      'http://localhost:3000/relay/devices/device-c/api/threads',
    ]);
    const scopes = exchange.mock.calls.map(call => (call as unknown as [Request, string | undefined])[1]);
    expect(scopes).toEqual(['thread-a', undefined, undefined]);
  });
});
