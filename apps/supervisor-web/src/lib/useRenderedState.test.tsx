import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useRenderedState } from './useRenderedState';

beforeEach(() => vi.useFakeTimers());
afterEach(() => { delete document.documentElement.dataset.pockymoeRendering; vi.useRealTimers(); });
it('coalesces visible progress bursts into one rendered frame', () => {
  let renders = 0;
  const { result } = renderHook(() => { renders++; return useRenderedState('initial'); });
  const before = renders;
  act(() => { for (let i = 0; i < 30; i++) result.current[1](`progress-${i}`); });
  expect(renders).toBe(before); expect(result.current[0]).toBe('initial');
  act(() => vi.advanceTimersByTime(20));
  expect(result.current[0]).toBe('progress-29'); expect(renders).toBe(before + 1);
});
it('buffers hidden progress without renders and reveals only the latest received value', () => {
  let renders = 0;
  const { result } = renderHook(() => { renders++; return useRenderedState('initial'); });
  act(() => { document.documentElement.dataset.pockymoeRendering = 'paused'; window.dispatchEvent(new Event('pockymoe-rendering-change')); });
  const before = renders;
  act(() => { for (let i = 0; i < 30; i++) result.current[1](`progress-${i}`); });
  expect(renders).toBe(before); expect(result.current[0]).toBe('initial');
  act(() => { delete document.documentElement.dataset.pockymoeRendering; window.dispatchEvent(new Event('pockymoe-rendering-change')); });
  expect(result.current[0]).toBe('progress-29'); expect(renders).toBe(before + 1);
});
it('catches up when a retained route becomes active', () => {
  const { result, rerender } = renderHook(({ active }) => useRenderedState('initial', active), { initialProps: { active: false } });
  act(() => result.current[1]('received'));
  expect(result.current[0]).toBe('initial');
  rerender({ active: true }); expect(result.current[0]).toBe('received');
});
it('keeps every functional update in order while the readout is paused', () => {
  const { result } = renderHook(() => useRenderedState(() => 0));
  act(() => { document.documentElement.dataset.pockymoeRendering = 'paused'; window.dispatchEvent(new Event('pockymoe-rendering-change')); });
  act(() => { for (let i = 0; i < 30; i++) result.current[1](value => value + 1); });
  expect(result.current[0]).toBe(0);
  act(() => { delete document.documentElement.dataset.pockymoeRendering; window.dispatchEvent(new Event('pockymoe-rendering-change')); });
  expect(result.current[0]).toBe(30);
});
