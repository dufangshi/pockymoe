import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';
import { deviceRequest, request } from '../../lib/api';
import { DesktopIndexStore, DesktopStoreContext } from '../data';
import type { DesktopIndex } from '../data/types';
import { clearMetrics, readMetrics, warmMetrics, writeMetrics } from './cache';
import type { StatsSummary } from './model';
import { useDeviceMetrics } from './useDeviceMetrics';

vi.mock('../../lib/api', () => ({ request: vi.fn(), deviceRequest: vi.fn(), fetchRelayPortal: vi.fn() }));
vi.mock('./cache', () => ({ clearMetrics: vi.fn(), readMetrics: vi.fn(), warmMetrics: vi.fn(), writeMetrics: vi.fn() }));
const summary: StatsSummary = { range: 'today', generatedAt: '2026-11-01T12:00:00Z', totals: { turns: 3, completed: 2, failed: 0, interrupted: 0, avgDurationMs: 1000, p50DurationMs: 1000, inputTokens: 20, outputTokens: 10, cachedTokens: 5, costUsd: .01 }, daily: [], byModel: [], byWorkspace: [], longestThreads: [], activeThreads: 1, runningTurns: 1 };
function setup(online: boolean) {
  const store = new DesktopIndexStore('account');
  const index: DesktopIndex = { accountId: 'account', devices: [{ id: 'device', name: 'Mac', connected: online, lastHeartbeatAt: null }], workspaces: [], threads: [], hydrated: true, refreshing: false, error: null };
  vi.spyOn(store, 'getSnapshot').mockReturnValue(index);
  const wrapper = ({ children }: { children: ReactNode }) => <DesktopStoreContext.Provider value={store}>{children}</DesktopStoreContext.Provider>;
  return renderHook(() => useDeviceMetrics('7d'), { wrapper });
}
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(warmMetrics).mockReturnValue({}); vi.mocked(readMetrics).mockResolvedValue({});
  vi.mocked(request).mockResolvedValue({ version: '0.12.59', capabilities: { statsSummary: true } });
  vi.mocked(deviceRequest).mockImplementation(async (_id, path) => path.includes('/management/') ? { runningVersion: '0.12.59' } : summary);
});
describe('device metrics transport and persistence', () => {
  it('restores offline summaries without probing or clearing them', async () => {
    vi.mocked(readMetrics).mockResolvedValue({ device: { summaries: { today: summary } } });
    const hook = setup(false);
    await waitFor(() => expect(hook.result.current.metrics.device?.summaries.today).toBe(summary));
    expect(request).not.toHaveBeenCalled(); expect(deviceRequest).not.toHaveBeenCalled();
    hook.unmount();
  });
  it('keeps cached values in place while revalidating and probes only a lightweight device-scoped health URL', async () => {
    const cached = { device: { summaries: { today: summary } } };
    vi.mocked(warmMetrics).mockReturnValue(cached); vi.mocked(readMetrics).mockResolvedValue(cached);
    const hook = setup(true);
    expect(hook.result.current.metrics.device?.summaries.today).toBe(summary);
    await waitFor(() => expect(hook.result.current.metrics.device?.summaries['7d']).toBe(summary));
    expect(request).toHaveBeenCalledWith('/relay/devices/device/healthz', expect.objectContaining({ cache: 'no-store', signal: expect.any(AbortSignal) }));
    expect(deviceRequest).toHaveBeenCalledWith('device', expect.stringContaining('/api/stats/summary?range=30d&tz='), expect.anything());
    expect(writeMetrics).toHaveBeenCalled(); hook.unmount();
  });
  it('does not request unsupported stats and retains a previous good summary after a 404', async () => {
    vi.mocked(request).mockResolvedValue({ capabilities: {} });
    const unsupported = setup(true);
    await waitFor(() => expect(unsupported.result.current.metrics.device?.partial).toBe(true));
    expect(vi.mocked(deviceRequest).mock.calls.some(([, path]) => path.includes('/stats/'))).toBe(false);
    unsupported.unmount();
    vi.mocked(readMetrics).mockResolvedValue({ device: { summaries: { today: summary } } });
    vi.mocked(request).mockResolvedValue({ capabilities: { statsSummary: true } });
    vi.mocked(deviceRequest).mockImplementation(async (_id, path) => { if (path.includes('/stats/')) throw { statusCode: 404 }; return {}; });
    const old = setup(true);
    await waitFor(() => expect(old.result.current.metrics.device?.partial).toBe(true));
    expect(old.result.current.metrics.device?.summaries.today).toBe(summary);
    old.unmount();
  });
  it('clears account-scoped cached metrics on logout', async () => {
    const hook = setup(false);
    await waitFor(() => expect(hook.result.current.hydrated).toBe(true));
    act(() => window.dispatchEvent(new Event('remote-codex:logout')));
    expect(clearMetrics).toHaveBeenCalledWith('account'); expect(hook.result.current.metrics).toEqual({});
    hook.unmount();
  });
  it('does not repopulate cleared metrics when an in-flight response arrives after logout', async () => {
    let resolveHealth!: (value: unknown) => void;
    vi.mocked(request).mockImplementation(() => new Promise(resolve => { resolveHealth = resolve; }));
    const hook = setup(true);
    await waitFor(() => expect(request).toHaveBeenCalled());
    await act(async () => {
      window.dispatchEvent(new Event('remote-codex:logout'));
      resolveHealth({ version: '0.12.59', capabilities: { statsSummary: true } });
    });
    expect(hook.result.current.metrics).toEqual({}); hook.unmount();
  });
  it('probes once per 30 seconds while visible and resumes when the page becomes visible', async () => {
    vi.useFakeTimers();
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    const hook = setup(true);
    try {
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });
      expect(request).toHaveBeenCalledTimes(1);
      await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
      expect(request).toHaveBeenCalledTimes(2);
      visibility.mockReturnValue('hidden');
      await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
      expect(request).toHaveBeenCalledTimes(2);
      visibility.mockReturnValue('visible');
      await act(async () => { document.dispatchEvent(new Event('visibilitychange')); });
      expect(request).toHaveBeenCalledTimes(3);
    } finally { hook.unmount(); visibility.mockRestore(); vi.useRealTimers(); }
  });
});
