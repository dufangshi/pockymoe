// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { matchesRelayWorkerRequest } from './relayWorkerMatching';

describe('relay service worker routing', () => {
  for (const configured of ['', 'https://relay.example.test']) {
    const web = 'https://web.example.test';
    const relay = configured || web;
    it(`intercepts device API media/download requests with ${configured || 'same origin'}`, () => {
      expect(matchesRelayWorkerRequest(new Request(`${relay}/relay/devices/d/api/files/content?path=x`), web, configured)).toBe(true);
      expect(matchesRelayWorkerRequest(new Request(`${relay}/relay/devices/d/api/threads/t/exports/html`), web, configured)).toBe(true);
      for (const path of ['/relay/portal', '/api/version', '/relay/devices/d/ws', '/relay/devices/d/api/transport/key', '/relay/devices/d/api/threads/t/transport/key'])
        expect(matchesRelayWorkerRequest(new Request(`${relay}${path}`), web, configured)).toBe(false);
      expect(matchesRelayWorkerRequest(new Request(`${relay}/relay/devices/d/api/files`, { headers: { 'x-rcd-key': 'already-encrypted' } }), web, configured)).toBe(false);
      expect(matchesRelayWorkerRequest(new Request('https://evil.example.test/relay/devices/d/api/files'), web, configured)).toBe(false);
      if (configured)
        expect(matchesRelayWorkerRequest(new Request(`${web}/relay/devices/d/api/files`), web, configured)).toBe(false);
    });
  }
});

it('lets a read the page makes for the worker pass through it', async () => {
  const { matchesRelayWorkerRequest, PAGE_FETCH_HEADER } = await import('./relayWorkerMatching');
  const url = 'https://relay.test/relay/devices/d1/api/threads/t1/assets/image?path=a.png';
  expect(matchesRelayWorkerRequest(new Request(url), 'https://relay.test', '')).toBe(true);
  expect(matchesRelayWorkerRequest(new Request(url, { headers: { [PAGE_FETCH_HEADER]: '1' } }), 'https://relay.test', '')).toBe(false);
  expect(PAGE_FETCH_HEADER.startsWith('x-rcd-')).toBe(true);
});
