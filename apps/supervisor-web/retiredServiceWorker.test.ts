import { expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import path from 'node:path';

type WorkerEvent = { waitUntil: (promise: Promise<unknown>) => void };

it('the old-origin worker activates immediately, unregisters and reloads its windows', async () => {
  const handlers = new Map<string, (event: WorkerEvent) => void>();
  const calls: string[] = [];
  const matchAll = vi.fn(async () => {
    calls.push('windows');
    return ['https://relay.example.test/devices', 'https://relay.example.test/threads/t'].map(url => ({
      url, navigate: vi.fn(async target => calls.push(target)),
    }));
  });
  runInNewContext(readFileSync(path.resolve(__dirname, '../../crates/relay/src/retired_service_worker.js'), 'utf8'), {
    self: {
      addEventListener: (name: string, handler: (event: WorkerEvent) => void) => handlers.set(name, handler),
      skipWaiting: async () => calls.push('skip'),
      registration: { unregister: async () => calls.push('unregister') },
      clients: { matchAll },
    },
  });
  for (const phase of ['install', 'activate']) {
    let completion: Promise<unknown> | undefined;
    handlers.get(phase)!({ waitUntil: promise => { completion = promise; } });
    expect(completion).toBeDefined();
    await completion;
  }
  expect(calls).toEqual(['skip', 'unregister', 'windows', 'https://relay.example.test/devices', 'https://relay.example.test/threads/t']);
  expect(matchAll).toHaveBeenCalledWith({ type: 'window', includeUncontrolled: true });
  expect(handlers.has('fetch')).toBe(false);
});
