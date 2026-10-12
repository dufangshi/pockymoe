import { expect, it, vi } from 'vitest';
import { staticHosting } from '../../staticHosting';

it.each(['', 'https://relay.example.test'])('emits a static CSP permitting WASM compilation without relaxing any other directive (%s)', relay => {
  const emitFile = vi.fn();
  const generate = staticHosting(relay, 'web.example.test').generateBundle;
  if (typeof generate !== 'function') throw new Error('Missing static header generator');
  generate.call({ emitFile } as never, {} as never, {} as never, false);
  const headers = emitFile.mock.calls.map(([file]) => file).find(file => file.fileName === '_headers').source as string;
  const csp = headers.match(/Content-Security-Policy: ([^\n]+)/)![1];
  const origin = relay, websocket = relay.replace('https:', 'wss:');
  expect(csp).toBe(`default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: ${origin}; media-src 'self' blob: ${origin}; font-src 'self' data:; connect-src 'self' ${origin} ${websocket}; worker-src 'self' blob:; frame-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self' ${origin}`);
  expect(headers).toContain('X-Content-Type-Options: nosniff');
  expect(headers).toContain('Referrer-Policy: no-referrer');
});
