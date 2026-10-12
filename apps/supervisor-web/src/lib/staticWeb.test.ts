import { describe, expect, it, vi } from 'vitest';
import { relayWorkerRegistration, requireTopLevelWeb } from './staticWeb';

describe('service worker registration', () => {
  it('registers the root worker in static mode without requiring response headers', () => {
    expect(relayWorkerRegistration('/assets/relayServiceWorker-hash.js', true)).toEqual({
      url: '/relayServiceWorker.js', options: { scope: '/', type: 'module' },
    });
  });
  it('keeps the bundled asset URL and registration options in same-origin mode', () => {
    expect(relayWorkerRegistration('/assets/relayServiceWorker-hash.js', false)).toEqual({
      url: '/assets/relayServiceWorker-hash.js', options: { scope: '/', type: 'module' },
    });
  });
});

describe('static frame protection', () => {
  it('allows a top-level page', () => expect(() => requireTopLevelWeb(window)).not.toThrow());
  it.each([false, true])('clears the document and stops initialization when framed (navigation blocked: %s)', blocked => {
    const clear = vi.fn();
    const replace = vi.fn(() => { if (blocked) throw new Error('blocked'); });
    const page = {
      top: { location: { replace } }, self: {},
      location: { href: 'https://web.example.test/devices' },
      document: { documentElement: { replaceChildren: clear } },
    } as unknown as Window;
    expect(() => requireTopLevelWeb(page)).toThrow('top-level window');
    expect(clear).toHaveBeenCalledOnce();
    expect(replace).toHaveBeenCalledWith('https://web.example.test/devices');
  });
});
