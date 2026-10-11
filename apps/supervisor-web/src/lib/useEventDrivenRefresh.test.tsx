import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useEventDrivenRefresh } from './useEventDrivenRefresh';

describe('event-driven refresh', () => {
  afterEach(() => vi.useRealTimers());

  it('spaces refreshes, runs once more after a request made in flight, and counts the fallback from outside reads', async () => {
    vi.useFakeTimers();
    let release!: () => void;
    const refresh = vi.fn(() => new Promise<void>(resolve => { release = resolve; }));
    const { result, unmount } = renderHook(() =>
      useEventDrivenRefresh({ enabled: true, refresh, fallbackMs: 60_000, coalesceMs: 100, minIntervalMs: 2_000 }));
    try {
      act(() => result.current.request());
      await act(() => vi.advanceTimersByTimeAsync(100));
      expect(refresh).toHaveBeenCalledTimes(1);
      act(() => { result.current.request(); result.current.request(); });
      await act(() => vi.advanceTimersByTimeAsync(500));
      expect(refresh).toHaveBeenCalledTimes(1);
      await act(async () => { release(); });
      // One follow-up for both requests, no sooner than the minimum interval.
      await act(() => vi.advanceTimersByTimeAsync(1_900));
      expect(refresh).toHaveBeenCalledTimes(1);
      await act(() => vi.advanceTimersByTimeAsync(200));
      expect(refresh).toHaveBeenCalledTimes(2);
      await act(async () => { release(); });
      await act(() => vi.advanceTimersByTimeAsync(50_000));
      act(() => result.current.markFresh());
      await act(() => vi.advanceTimersByTimeAsync(59_000));
      expect(refresh).toHaveBeenCalledTimes(2);
      await act(() => vi.advanceTimersByTimeAsync(1_000));
      expect(refresh).toHaveBeenCalledTimes(3);
    } finally { unmount(); }
  });

  it('does nothing while disabled', async () => {
    vi.useFakeTimers();
    const refresh = vi.fn(async () => {});
    const { result, unmount } = renderHook(() => useEventDrivenRefresh({ enabled: false, refresh, fallbackMs: 1_000 }));
    act(() => result.current.request(0));
    await act(() => vi.advanceTimersByTimeAsync(5_000));
    expect(refresh).not.toHaveBeenCalled();
    unmount();
  });
});
