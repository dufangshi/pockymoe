import { afterEach, describe, expect, it, vi } from 'vitest';
import { relayOrigin, relaySocketUrl, relayUrl, relayFetchInit } from './relayOrigin';

afterEach(() => vi.unstubAllEnvs());
describe('relay network origin', () => {
  it('preserves relative single-origin API and media URLs', () => {
    expect(relayOrigin('https://web.example.test', '')).toBe('https://web.example.test');
    expect(relayUrl('/relay/devices/a/api/files?path=a%20b', 'https://web.example.test', '')).toBe('/relay/devices/a/api/files?path=a%20b');
    expect(relaySocketUrl('/relay/ws?threadId=t', 'https://web.example.test', '')).toBe('wss://web.example.test/relay/ws?threadId=t');
  });
  it('routes HTTP, media and WebSockets to the configured relay while preserving encoding', () => {
    const web = 'https://web.example.test';
    const relay = 'https://relay.example.test:8443/';
    expect(relayUrl('/relay/devices/a%2Fb/api/files?path=a%20b#part', web, relay)).toBe('https://relay.example.test:8443/relay/devices/a%2Fb/api/files?path=a%20b#part');
    expect(relayUrl(`${web}/relay/portal`, web, relay)).toBe('https://relay.example.test:8443/relay/portal');
    expect(relaySocketUrl('/relay/devices/a/ws?threadId=t', web, relay)).toBe('wss://relay.example.test:8443/relay/devices/a/ws?threadId=t');
    expect(relayUrl('/api/version', web, relay)).toBe('/api/version');
    expect(relayUrl('/devices/a/threads/t', web, relay)).toBe('/devices/a/threads/t');
    expect(relayUrl('https://external.test/relay/portal', web, relay)).toBe('https://external.test/relay/portal');
  });
  it('uses the build-time origin and supports localhost development', () => {
    vi.stubEnv('VITE_POCKYMOE_RELAY_ORIGIN', 'http://localhost:4189');
    expect(relayUrl('/relay/portal', 'http://localhost:4188')).toBe('http://localhost:4189/relay/portal');
    expect(relaySocketUrl('/relay/ws', 'http://localhost:4188')).toBe('ws://localhost:4189/relay/ws');
  });
  it('includes cookies on relay fetches while preserving unrelated request options', () => {
    vi.stubEnv('VITE_POCKYMOE_RELAY_ORIGIN', 'https://relay.example.test');
    expect(relayFetchInit('https://relay.example.test/relay/portal', { method: 'POST' })).toEqual({ method: 'POST', credentials: 'include' });
    expect(relayFetchInit('https://external.test/image', { credentials: 'omit' })).toEqual({ credentials: 'omit' });
  });
  it('rejects insecure or non-origin configuration', () => {
    for (const value of ['http://relay.example.test', 'https://relay.example.test/path', 'https://user@relay.example.test', 'https://relay.example.test?query=1', 'https://relay.example.test#fragment'])
      expect(() => relayOrigin('https://web.example.test', value)).toThrow();
  });
});
