// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import type { PockymoeHost } from '../desktop/host';
vi.mock('@pockymoe/thread-ui/i18n', () => ({ translate: () => 'Unable to load Relay' }));
afterEach(() => { delete window.pockymoeHost; vi.resetModules(); vi.unstubAllEnvs(); });
it('initializes all page Relay HTTP, WebSocket, cookie and media routes from the shell pair', async () => {
  const { initializeMobileConnection } = await import('./connection');
  const origins = await import('../lib/relayOrigin');
  vi.stubEnv('VITE_POCKYMOE_RELAY_ORIGIN', 'https://build.example.com');
  const get = vi.fn(async () => ({ webOrigin: location.origin, relayOrigin: 'https://self-host.example.com' }));
  window.pockymoeHost = { capabilities: async () => ({ surface: 'mobile' }), settings: { get } } as unknown as PockymoeHost;
  await initializeMobileConnection(); await initializeMobileConnection();
  expect(get).toHaveBeenCalledTimes(1);
  expect(origins.relayUrl('relay/auth/session')).toBe('https://self-host.example.com/relay/auth/session');
  expect(origins.relayUrl('relay/account/notifications/native')).toBe('https://self-host.example.com/relay/account/notifications/native');
  expect(origins.relaySocketUrl('relay/devices/d/ws')).toBe('wss://self-host.example.com/relay/devices/d/ws');
  expect(origins.relayFetchInit('relay/auth/mobile/exchange')).toMatchObject({ credentials: 'include' });
  const { matchesRelayWorkerRequest } = await import('../lib/relayWorkerMatching');
  expect(matchesRelayWorkerRequest(new Request('https://self-host.example.com/relay/devices/d/api/files'))).toBe(true);
});
it.each(['', 'http://insecure.example.com', 'https://self-host.example.com/path'])('blocks invalid shell configuration instead of falling back: %s', async relayOrigin => {
  const { initializeMobileConnection } = await import('./connection');
  window.pockymoeHost = { capabilities: async () => ({ surface: 'mobile' }), settings: { get: async () => ({ webOrigin: location.origin, relayOrigin }) } } as unknown as PockymoeHost;
  await expect(initializeMobileConnection()).rejects.toThrow();
  const origins = await import('../lib/relayOrigin'); expect(() => origins.relayUrl('relay/auth/session')).toThrow();
});
it('does not query settings or override normal browser/desktop connection configuration', async () => {
  const { initializeMobileConnection } = await import('./connection');
  const get = vi.fn(); window.pockymoeHost = { capabilities: async () => ({ surface: 'desktop' }), settings: { get } } as unknown as PockymoeHost;
  await initializeMobileConnection(); expect(get).not.toHaveBeenCalled();
});
