import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { request } from './api';
import { useReasoningSummaries } from './useReasoningSummaries';

const relay = vi.hoisted(() => ({ active: true }));
vi.mock('./api', () => ({ relayModeActive: () => relay.active, request: vi.fn() }));
const LOCAL = 'remote-codex-show-reasoning-summaries';
const patches = () => vi.mocked(request).mock.calls.filter(([, init]) => init?.method === 'PATCH').map(([, init]) => JSON.parse(String(init!.body)));

beforeEach(() => { relay.active = true; localStorage.clear(); });
afterEach(() => { vi.resetAllMocks(); });

it('follows the account in every browser, and again when coming back to the window', async () => {
  vi.mocked(request).mockResolvedValueOnce({ showReasoningSummaries: true }).mockResolvedValue({ showReasoningSummaries: false });
  const hook = renderHook(useReasoningSummaries);
  expect(hook.result.current[0]).toBe(false);
  await waitFor(() => expect(hook.result.current[0]).toBe(true));
  expect(localStorage.getItem(LOCAL)).toBe('true');
  act(() => { window.dispatchEvent(new Event('focus')); });
  await waitFor(() => expect(hook.result.current[0]).toBe(false));
  expect(patches()).toEqual([]);
});

it('gives an account that never chose the setting this browser already had on', async () => {
  localStorage.setItem(LOCAL, 'true');
  vi.mocked(request).mockResolvedValue({ showReasoningSummaries: null });
  const hook = renderHook(useReasoningSummaries);
  await waitFor(() => expect(patches()).toEqual([{ showReasoningSummaries: true }]));
  expect(hook.result.current[0]).toBe(true);
});

it('keeps a change the account did not receive and sends it again before reading', async () => {
  vi.mocked(request).mockResolvedValueOnce({ showReasoningSummaries: false });
  const hook = renderHook(useReasoningSummaries);
  await waitFor(() => expect(request).toHaveBeenCalledTimes(1));
  vi.mocked(request).mockRejectedValueOnce(new Error('offline'));
  act(() => { hook.result.current[1](true); });
  expect(hook.result.current[0]).toBe(true);
  await waitFor(() => expect(patches()).toHaveLength(1));
  // Back online: the unsaved change goes first, so the read cannot undo it.
  vi.mocked(request).mockResolvedValue({ showReasoningSummaries: true });
  act(() => { window.dispatchEvent(new Event('focus')); });
  await waitFor(() => expect(patches()).toHaveLength(2));
  await waitFor(() => expect(vi.mocked(request).mock.calls.at(-1)?.[1]?.method).toBeUndefined());
  expect(hook.result.current[0]).toBe(true);
});

it('stays in the browser on a relay from before the preference, and in local mode', async () => {
  localStorage.setItem(LOCAL, 'true');
  vi.mocked(request).mockResolvedValue({ sendShortcut: 'enter' });
  const old = renderHook(useReasoningSummaries);
  await waitFor(() => expect(request).toHaveBeenCalledTimes(1));
  expect(old.result.current[0]).toBe(true);
  expect(patches()).toEqual([]);
  old.unmount();

  vi.mocked(request).mockClear();
  relay.active = false;
  const local = renderHook(useReasoningSummaries);
  act(() => { local.result.current[1](false); });
  expect(local.result.current[0]).toBe(false);
  expect(localStorage.getItem(LOCAL)).toBe('false');
  expect(request).not.toHaveBeenCalled();
});
