import { renderHook, act } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { ThreadDto } from '@pockymoe/shared';
import { useThreadTabStatus } from './useThreadTabStatus';
const favicon = () =>
  decodeURIComponent(
    document.querySelector<HTMLLinkElement>('link[type="image/svg+xml"]')!.href,
  );
afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
});
it('keeps a background completion unread across reload, then marks it read on focus; running wins', () => {
  window.history.replaceState({}, '', '/threads/thread-a');
  vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
  const focus = vi.spyOn(document, 'hasFocus').mockReturnValue(false);
  const base = {
    id: 'thread-a',
    title: 'Compiler',
    status: 'running',
    lastTurnCompletedAt: null,
  } as ThreadDto;
  const hook = renderHook(({ thread }) => useThreadTabStatus(thread), {
    initialProps: { thread: base },
  });
  expect(document.title).toBe('Compiler');
  expect(favicon()).toContain('M16 7a9 9');
  const completed = {
    ...base,
    status: 'idle',
    lastTurnCompletedAt: '2026-09-12T00:00:00Z',
  } as ThreadDto;
  hook.rerender({ thread: completed });
  expect(document.title).toBe('Compiler');
  expect(favicon()).toContain('fill="#2fbf71"');
  hook.unmount();
  const reloaded = renderHook(() => useThreadTabStatus(completed));
  expect(document.title).toBe('Compiler');
  expect(favicon()).toContain('fill="#2fbf71"');
  focus.mockReturnValue(true);
  act(() => {
    window.dispatchEvent(new Event('focus'));
  });
  expect(document.title).toBe('Compiler');
  expect(favicon()).toContain('r="5.5"');
  reloaded.unmount();
  const running = renderHook(() =>
    useThreadTabStatus({ ...completed, status: 'running' }),
  );
  expect(document.title).toBe('Compiler');
  expect(favicon()).toContain('M16 7a9 9');
  running.unmount();
  expect(document.querySelector('link[type="image/svg+xml"]')).toBeNull();
});
it('draws failed, interrupted and waiting threads with their own status shapes', () => {
  window.history.replaceState({}, '', '/threads/thread-b');
  vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
  vi.spyOn(document, 'hasFocus').mockReturnValue(true);
  const thread = { id: 'thread-b', title: 'Build', status: 'failed', lastTurnCompletedAt: '2026-09-12T00:00:00Z' } as ThreadDto;
  const hook = renderHook(({ value, waiting }) => useThreadTabStatus(value, waiting), { initialProps: { value: thread, waiting: false } });
  expect(favicon()).toContain('M16 7l9 9-9 9-9-9z');
  hook.rerender({ value: { ...thread, status: 'interrupted' } as ThreadDto, waiting: false });
  expect(favicon()).toContain('<rect x="10"');
  hook.rerender({ value: { ...thread, status: 'running' } as ThreadDto, waiting: true });
  expect(favicon()).toContain('fill="#edb75d"');
  hook.unmount();
});
