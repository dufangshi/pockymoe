import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { request } from '../../lib/api';
import { clearMetrics, readMetrics, warmMetrics, writeMetrics } from './cache';
import { useDeviceStats } from './useDeviceStats';
import type { StatsSummary } from './model';

vi.mock('../../lib/api', () => ({ request: vi.fn() }));
vi.mock('./cache', () => ({ clearMetrics: vi.fn(), readMetrics: vi.fn(), warmMetrics: vi.fn(), writeMetrics: vi.fn() }));
const devices = [{ id: 'mac', name: 'Mac', connected: true, lastHeartbeatAt: null }];
const summary = { range: 'today', generatedAt: new Date().toISOString(), totals: { turns: 10 }, daily: [] } as unknown as StatsSummary;
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(warmMetrics).mockReturnValue({}); vi.mocked(readMetrics).mockResolvedValue({});
  vi.mocked(request).mockImplementation(async (path: RequestInfo) => String(path).endsWith('/healthz') ? { version: '0.13.41', capabilities: { statsSummary: true } } : String(path).includes('management') ? { runningVersion: '0.13.41' } : summary);
});
it('keeps good cached values in place and scopes every request to the explicit device', async () => {
  const cached = { mac: { summaries: { today: summary } } };
  vi.mocked(warmMetrics).mockReturnValue(cached); vi.mocked(readMetrics).mockResolvedValue(cached);
  const fallback = vi.fn().mockResolvedValue(undefined);
  const hook = renderHook(() => useDeviceStats('owner', devices, '7d', '', fallback));
  expect(hook.result.current.metrics.mac?.summaries.today).toBe(summary);
  await waitFor(() => expect(hook.result.current.metrics.mac?.summaries['7d']).toBe(summary));
  expect(vi.mocked(request).mock.calls.every(([path]: Parameters<typeof request>) => String(path).startsWith('relay/devices/mac/'))).toBe(true);
  expect(fallback).not.toHaveBeenCalled(); expect(writeMetrics).toHaveBeenCalled(); hook.unmount();
});
it('uses the shared activity refresh when summaries are unsupported or 404, retaining last good stats', async () => {
  vi.mocked(readMetrics).mockResolvedValue({ mac: { summaries: { today: summary } } });
  vi.mocked(request).mockImplementation(async (path: RequestInfo) => {
    if (String(path).endsWith('/healthz')) return { capabilities: { statsSummary: true } };
    if (String(path).includes('/stats/')) throw { statusCode: 404 };
    return {};
  });
  const fallback = vi.fn().mockResolvedValue(undefined);
  const hook = renderHook(() => useDeviceStats('owner', devices, '7d', '', fallback));
  await waitFor(() => expect(fallback).toHaveBeenCalledWith('mac'));
  expect(hook.result.current.metrics.mac?.partial).toBe(true);
  expect(hook.result.current.metrics.mac?.summaries.today).toBe(summary); hook.unmount();
});
it('restores offline snapshots without network reads', async () => {
  vi.mocked(readMetrics).mockResolvedValue({ mac: { summaries: { today: summary } } });
  const hook = renderHook(() => useDeviceStats('owner', [{ ...devices[0]!, connected: false }], 'today', '', vi.fn()));
  await waitFor(() => expect(hook.result.current.metrics.mac?.summaries.today).toBe(summary));
  expect(request).not.toHaveBeenCalled(); hook.unmount();
});
it('stops late responses and clears account metrics on the upstream logout event', async () => {
  let resolve!: (value: unknown) => void;
  vi.mocked(request).mockImplementation(() => new Promise(done => { resolve = done; }));
  const hook = renderHook(() => useDeviceStats('owner', devices, 'today', '', vi.fn()));
  await waitFor(() => expect(request).toHaveBeenCalled());
  await act(async () => { window.dispatchEvent(new Event('pockymoe-account-logout')); resolve({ capabilities: { statsSummary: true } }); });
  expect(clearMetrics).toHaveBeenCalledWith('owner'); expect(hook.result.current.metrics).toEqual({}); hook.unmount();
});
it('pauses hidden probes, uses 60s stats and 30s latency, and debounces completions without an extra initial read', async () => {
  vi.useFakeTimers();
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
  const hook = renderHook(({ completed }) => useDeviceStats('owner', devices, 'today', completed, vi.fn()), { initialProps: { completed: '' } });
  const statReads = () => vi.mocked(request).mock.calls.filter(([path]: Parameters<typeof request>) => String(path).includes('/stats/')).length;
  const healthReads = () => vi.mocked(request).mock.calls.filter(([path]: Parameters<typeof request>) => String(path).endsWith('/healthz')).length;
  try {
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(statReads()).toBe(3); expect(healthReads()).toBe(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(statReads()).toBe(3); expect(healthReads()).toBe(2);
    hidden.mockReturnValue(true);
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(statReads()).toBe(3); expect(healthReads()).toBe(2);
    hidden.mockReturnValue(false);
    hook.rerender({ completed: '2026-11-01T00:00:00Z' });
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(statReads()).toBe(6);
  } finally { hook.unmount(); hidden.mockRestore(); vi.useRealTimers(); }
});
it('keeps All time unsupported separate from finite summaries and requests the lifetime fallback', async () => {
  vi.mocked(request).mockImplementation(async (path: RequestInfo) => {
    if (String(path).endsWith('/healthz')) return { capabilities: { statsSummary: true } };
    if (String(path).includes('range=all')) throw { statusCode: 400 };
    return summary;
  });
  const fallback = vi.fn().mockResolvedValue(undefined);
  const hook = renderHook(() => useDeviceStats('owner', devices, 'all', '', fallback));
  await waitFor(() => expect(fallback).toHaveBeenCalledWith('mac'));
  expect(hook.result.current.metrics.mac?.allUnsupported).toBe(true);
  expect(hook.result.current.metrics.mac?.summaries.today).toBe(summary);
  expect(hook.result.current.metrics.mac?.unavailable).toBe(false); hook.unmount();
});
it('caches origins separately and rejects legacy responses that silently ignore the origin query', async () => {
  let echo = false;
  const base = { ...summary, byOrigin: [{ origin: 'guest', turns: 1, tokens: 10, costUsd: 1 }] };
  vi.mocked(request).mockImplementation(async (path: RequestInfo) => {
    if (String(path).endsWith('/healthz')) return { capabilities: { statsSummary: true } };
    if (String(path).includes('origin=guest')) return { ...base, origin: echo ? 'guest' : 'all' };
    return base;
  });
  const hook = renderHook(() => useDeviceStats('owner', devices, '7d', '', vi.fn(), 'guest'));
  await waitFor(() => expect(request).toHaveBeenCalledWith(expect.stringContaining('origin=guest'), expect.anything()));
  expect(hook.result.current.metrics.mac?.originSummaries?.guest).toBeUndefined();
  echo = true; act(() => hook.result.current.refresh());
  await waitFor(() => expect(hook.result.current.metrics.mac?.originSummaries?.guest?.['7d']?.origin).toBe('guest'));
  expect(hook.result.current.metrics.mac?.summaries['7d']).toBe(base); hook.unmount();
});
