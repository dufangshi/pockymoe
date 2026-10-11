import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fetchThreads } from '../lib/api';
import { THREAD_LIST_POLL_INTERVAL_MS, useThreadListPolling } from './useThreadListPolling';

vi.mock('../lib/api', () => ({ fetchThreads: vi.fn() }));

function setVisibility(state: 'hidden' | 'visible') {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state });
  document.dispatchEvent(new Event('visibilitychange'));
}

describe('thread group polling', () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.mocked(fetchThreads).mockReset();
    setVisibility('visible');
  });
  it('keeps agent threads in repeated detail-page snapshots, but leaves root-only listings unchanged', async () => {
    vi.useFakeTimers();
    vi.mocked(fetchThreads).mockResolvedValue([]);
    const setThreads = vi.fn();
    const { rerender, unmount } = renderHook(({ includeAgentThreads }) =>
      useThreadListPolling({ enabled: true, setThreads, intervalMs: 1000, includeAgentThreads }),
      { initialProps: { includeAgentThreads: true } });
    try {
      await act(() => vi.advanceTimersByTimeAsync(2000));
      expect(vi.mocked(fetchThreads).mock.calls).toEqual([[true], [true]]);
      expect(setThreads).toHaveBeenCalledTimes(2);
      rerender({ includeAgentThreads: false });
      await act(() => vi.advanceTimersByTimeAsync(1000));
      expect(fetchThreads).toHaveBeenLastCalledWith(false);
    } finally { unmount(); }
  });

  it('refreshes on announced changes, coalesces bursts and otherwise waits for the slow fallback', async () => {
    vi.useFakeTimers();
    vi.mocked(fetchThreads).mockResolvedValue([]);
    const setThreads = vi.fn();
    const { result, unmount } = renderHook(() => useThreadListPolling({ enabled: true, setThreads, includeAgentThreads: true }));
    try {
      await act(() => vi.advanceTimersByTimeAsync(30_000));
      expect(fetchThreads).not.toHaveBeenCalled();
      act(() => { result.current.refresh(); result.current.refresh(); result.current.refresh(); });
      await act(() => vi.advanceTimersByTimeAsync(1000));
      expect(fetchThreads).toHaveBeenCalledTimes(1);
      // The fallback counts from the last refresh.
      await act(() => vi.advanceTimersByTimeAsync(THREAD_LIST_POLL_INTERVAL_MS - 2000));
      expect(fetchThreads).toHaveBeenCalledTimes(1);
      await act(() => vi.advanceTimersByTimeAsync(2000));
      expect(fetchThreads).toHaveBeenCalledTimes(2);
    } finally { unmount(); }
  });

  it('polls faster without a live socket and never while the tab is hidden', async () => {
    vi.useFakeTimers();
    vi.mocked(fetchThreads).mockResolvedValue([]);
    const setThreads = vi.fn();
    const { result, unmount } = renderHook(() => useThreadListPolling({ enabled: true, setThreads, live: false }));
    try {
      await act(() => vi.advanceTimersByTimeAsync(10_000));
      expect(fetchThreads).toHaveBeenCalledTimes(1);
      setVisibility('hidden');
      act(() => result.current.refresh());
      await act(() => vi.advanceTimersByTimeAsync(60_000));
      expect(fetchThreads).toHaveBeenCalledTimes(1);
      // The deferred change is read as soon as the tab is visible again.
      act(() => setVisibility('visible'));
      await act(() => vi.advanceTimersByTimeAsync(10));
      expect(fetchThreads).toHaveBeenCalledTimes(2);
    } finally { unmount(); }
  });
});
