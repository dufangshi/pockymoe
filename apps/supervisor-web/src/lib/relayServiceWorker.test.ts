// @vitest-environment node
import { afterEach, expect, it, vi } from 'vitest';
const { relayWorkerFetch } = vi.hoisted(() => ({ relayWorkerFetch: vi.fn() }));
vi.mock('./relayServiceWorkerFetch', () => ({ relayWorkerFetch }));
vi.mock('./notificationWorker', () => ({ installPushHandlers: vi.fn() }));
vi.mock('./relayTransportCrypto', () => ({ clearTransportKeyCache: vi.fn(), clearAllTransportKeys: vi.fn(), setTransportReporter: vi.fn() }));
vi.mock('@pockymoe/thread-ui/i18n', () => ({ setLocale: vi.fn() }));
afterEach(() => { vi.resetModules(); vi.unstubAllGlobals(); vi.clearAllMocks(); });
it('routes each mobile page through its own Relay and never another controlled page', async () => {
  const listeners = new Map<string, (event: any) => void>();
  const clients = new Map<string, any>();
  const forwarded: string[] = [];
  const page = (id: string) => ({ id, url: 'https://app.example.com/relay-devices', postMessage: (message: any, ports: any[]) => { forwarded.push(id); ports[0].postMessage({ status: 200, headers: [], body: new ArrayBuffer(0) }); } });
  clients.set('one', page('one')); clients.set('two', page('two'));
  vi.stubGlobal('self', { location: { origin: 'https://app.example.com' }, addEventListener: (type: string, listener: any) => listeners.set(type, listener), clients: { get: async (id: string) => clients.get(id), matchAll: async () => [...clients.values()] } });
  class Channel {
    port1 = { onmessage: null as any };
    port2 = { postMessage: (data: unknown) => queueMicrotask(() => this.port1.onmessage({ data })) };
  }
  vi.stubGlobal('MessageChannel', Channel);
  await import('./relayServiceWorker');
  for (const [id, origin] of [['one', 'https://first.example.com'], ['two', 'https://second.example.com']] as const) {
    const ack = vi.fn(); listeners.get('message')!({ data: { type: 'pockymoe-page-relay', origin }, source: clients.get(id), ports: [{ postMessage: ack }] });
    expect(ack).toHaveBeenCalledWith({ ok: true });
  }
  async function read(clientId: string, origin: string) {
    let response: Promise<Response> | undefined;
    listeners.get('fetch')!({ clientId, request: new Request(`${origin}/relay/devices/d/api/files`), respondWith: (value: Promise<Response>) => { response = value; } });
    return response;
  }
  expect((await read('one', 'https://first.example.com'))?.status).toBe(200);
  expect((await read('two', 'https://second.example.com'))?.status).toBe(200);
  expect(await read('one', 'https://second.example.com')).toBeUndefined();
  expect(forwarded).toEqual(['one', 'two']); expect(relayWorkerFetch).not.toHaveBeenCalled();
  clients.delete('one');
  expect((await read('one', 'https://first.example.com'))?.status).toBe(503);
  expect(forwarded).toEqual(['one', 'two']);
  const ack = vi.fn(); listeners.get('message')!({ data: { type: 'pockymoe-page-relay', origin: 'https://evil.example.com' }, source: { id: 'two', url: 'https://other.example.com' }, ports: [{ postMessage: ack }] }); expect(ack).not.toHaveBeenCalled();
});
