import { beforeEach, expect, it, vi } from 'vitest';

vi.mock('./relayTransportCrypto', async (actual) => ({
  ...(await actual<typeof import('./relayTransportCrypto')>()),
  exchange: vi.fn(),
}));
import { exchange } from './relayTransportCrypto';
import { AccountTrustError } from './accountVaultState';
import { relayWorkerFetch } from './relayServiceWorkerFetch';

const url = 'https://relay.test/relay/devices/d1/api/threads/t1/assets/image?path=a.png&variant=thumbnail';
const page = vi.fn(async () => new Response('from page', { status: 200 }));
beforeEach(() => { vi.mocked(exchange).mockReset(); page.mockClear(); });

it('asks the page when the worker reads the browser key as missing (Safari)', async () => {
  vi.mocked(exchange).mockRejectedValue(new AccountTrustError('accountUnlockRequired'));
  const response = await relayWorkerFetch(new Request(url), undefined, page);
  expect(await response.text()).toBe('from page');
  expect(page).toHaveBeenCalledTimes(1);
});

it('asks the page when the device refuses the worker', async () => {
  vi.mocked(exchange).mockResolvedValue({ response: new Response('{}', { status: 401 }) } as never);
  expect((await relayWorkerFetch(new Request(url), undefined, page)).status).toBe(200);
});

it('keeps the worker answer when it works, and never repeats a write', async () => {
  vi.mocked(exchange).mockResolvedValue({ response: new Response('image', { status: 200 }) } as never);
  expect(await (await relayWorkerFetch(new Request(url), undefined, page)).text()).toBe('image');
  vi.mocked(exchange).mockRejectedValue(new AccountTrustError('accountUnlockRequired'));
  const write = await relayWorkerFetch(new Request(url, { method: 'POST', body: 'x' }), undefined, page);
  expect(write.status).toBe(401);
  expect(page).not.toHaveBeenCalled();
});
