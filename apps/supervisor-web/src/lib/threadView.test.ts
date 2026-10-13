import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { THREAD_VIEW_HIDDEN_MS, ThreadViewReporter, useThreadView } from './threadView';

const socket = (readyState = 1) => ({ readyState, send: vi.fn() }) as unknown as WebSocket & { send: ReturnType<typeof vi.fn> };
const sent = (target: { send: ReturnType<typeof vi.fn> }) =>
  target.send.mock.calls.map(([body]) => (JSON.parse(String(body)) as { threadIds: string[] }).threadIds);
let visibility: DocumentVisibilityState = 'visible';
const setVisibility = (state: DocumentVisibilityState) => {
  visibility = state;
  document.dispatchEvent(new Event('visibilitychange'));
};

beforeEach(() => {
  vi.useFakeTimers();
  visibility = 'visible';
  vi.spyOn(document, 'visibilityState', 'get').mockImplementation(() => visibility);
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('sends the full set when it changes and nothing when it does not', () => {
  const current = socket();
  const reporter = new ThreadViewReporter(() => current);
  reporter.setThreads(['b', 'a']);
  reporter.setThreads(['a', 'b', 'a']);
  reporter.setThreads(['a']);
  reporter.setThreads([]);
  expect(sent(current)).toEqual([['a', 'b'], ['a'], []]);
  expect(JSON.parse(String(current.send.mock.calls[0]![0]))).toEqual({ type: 'thread.view', threadIds: ['a', 'b'] });
});

it('waits for an open socket and resends on every new connection', () => {
  let current = socket(0);
  const reporter = new ThreadViewReporter(() => current);
  reporter.setThreads(['a']);
  expect(current.send).not.toHaveBeenCalled();
  (current as { readyState: number }).readyState = 1;
  reporter.socketOpened();
  expect(sent(current)).toEqual([['a']]);
  current = socket();
  reporter.socketOpened();
  expect(sent(current)).toEqual([['a']]);
});

it('does not announce an empty set to a fresh connection', () => {
  const current = socket();
  const reporter = new ThreadViewReporter(() => current);
  reporter.socketOpened();
  reporter.setThreads([]);
  expect(current.send).not.toHaveBeenCalled();
});

it('clears the set after a minute hidden and restores it when visible again', () => {
  const current = socket();
  const ref = { current: current as WebSocket | null };
  renderHook(() => useThreadView(ref, ['a', 'b']));
  expect(sent(current)).toEqual([['a', 'b']]);
  act(() => setVisibility('hidden'));
  act(() => vi.advanceTimersByTime(THREAD_VIEW_HIDDEN_MS - 1));
  expect(sent(current)).toEqual([['a', 'b']]);
  act(() => vi.advanceTimersByTime(1));
  expect(sent(current)).toEqual([['a', 'b'], []]);
  act(() => setVisibility('visible'));
  expect(sent(current)).toEqual([['a', 'b'], [], ['a', 'b']]);
});

it('keeps a briefly hidden tab displayed', () => {
  const current = socket();
  const ref = { current: current as WebSocket | null };
  renderHook(() => useThreadView(ref, ['a']));
  act(() => setVisibility('hidden'));
  act(() => vi.advanceTimersByTime(THREAD_VIEW_HIDDEN_MS / 2));
  act(() => setVisibility('visible'));
  act(() => vi.advanceTimersByTime(THREAD_VIEW_HIDDEN_MS));
  expect(sent(current)).toEqual([['a']]);
});

it('reports set changes from the hook and resends after a reconnect', () => {
  const first = socket();
  const ref = { current: first as WebSocket | null };
  const { result, rerender } = renderHook(({ ids }) => useThreadView(ref, ids), { initialProps: { ids: ['a'] } });
  rerender({ ids: ['a'] });
  rerender({ ids: ['a', 'split'] });
  expect(sent(first)).toEqual([['a'], ['a', 'split']]);
  const second = socket();
  ref.current = second;
  act(() => result.current.socketOpened());
  expect(sent(second)).toEqual([['a', 'split']]);
});
