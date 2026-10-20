import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { request } from './api';

const busy = () => Response.json({ code: 'service_unavailable', message: 'device is offline' }, { status: 503 });
beforeEach(() => {
  localStorage.clear();
  vi.useFakeTimers();
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

it('asks once more when a read meets a device that is briefly unavailable', async () => {
  const fetch = vi.fn().mockResolvedValueOnce(busy()).mockResolvedValueOnce(Response.json({ ok: true }));
  vi.stubGlobal('fetch', fetch);
  const result = request<{ ok: boolean }>('api/threads');
  await vi.advanceTimersByTimeAsync(1000);
  await expect(result).resolves.toEqual({ ok: true });
  expect(fetch).toHaveBeenCalledTimes(2);
});

it('never repeats a write', async () => {
  const fetch = vi.fn().mockResolvedValue(busy());
  vi.stubGlobal('fetch', fetch);
  const result = request('api/threads', { method: 'POST', body: '{}' });
  const settled = expect(result).rejects.toMatchObject({ statusCode: 503 });
  await vi.advanceTimersByTimeAsync(1000);
  await settled;
  expect(fetch).toHaveBeenCalledTimes(1);
});
