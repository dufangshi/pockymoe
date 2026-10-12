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
