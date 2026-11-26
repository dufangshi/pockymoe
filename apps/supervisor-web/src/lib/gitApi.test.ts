import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError, deviceRequest } from './api';
import { fetchThreadGitCommits, fetchThreadGitDiff, fetchThreadGitStatus, fetchThreadWorkingContext, gitWrite } from './gitApi';
vi.mock('./api', async importOriginal => ({ ...await importOriginal<typeof import('./api')>(), deviceRequest: vi.fn().mockResolvedValue({}) }));
beforeEach(() => { vi.mocked(deviceRequest).mockReset().mockResolvedValue({}); });
afterEach(() => { vi.useRealTimers(); });

it('keeps all Git reads on the selected device and thread scope, encoding paths and refs', async () => {
  const signal = new AbortController().signal;
  await fetchThreadWorkingContext('thread/id', 'device-b', signal);
  await fetchThreadGitStatus('thread/id', 'copy&id', 'device-b', signal);
  await fetchThreadGitDiff('thread/id', 'copy&id', { area: 'branch', base: 'main~2', path: 'dir/a & b.rs' }, 'device-b', signal);
  await fetchThreadGitCommits('thread/id', 'copy&id', 30, 'device-b', signal);
  const calls = vi.mocked(deviceRequest).mock.calls;
  expect(calls).toHaveLength(4);
  for (const call of calls) {
    expect(call[0]).toBe('device-b');
    expect(call[2]).toEqual({ cache: 'no-store', signal });
    expect(call[3]).toBe('thread/id');
    expect(String(call[1])).toContain('api/threads/thread%2Fid/');
  }
  const diff = new URL(String(calls[2]![1]), 'http://localhost');
  expect(Object.fromEntries(diff.searchParams)).toEqual({ workingCopyId: 'copy&id', area: 'branch', path: 'dir/a & b.rs', base: 'main~2' });
  expect(new URL(String(calls[3]![1]), 'http://localhost').searchParams.get('offset')).toBe('30');
});


it.each(['checkpoints/restore', 'checkpoints/undo'])('retries %s once after a pre-write harness busy refusal', async action => {
  vi.useFakeTimers();
  const error = new ApiError(409, { code: 'checkpoint_busy', message: 'Harness operation is finishing' });
  vi.mocked(deviceRequest).mockRejectedValueOnce(error).mockResolvedValueOnce({ restored: true });
  const promise = gitWrite('thread', action, { turnId: 'turn' }, 'device');
  await vi.advanceTimersByTimeAsync(749);
  expect(deviceRequest).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1);
  expect(await promise).toEqual({ restored: true });
  expect(deviceRequest).toHaveBeenCalledTimes(2);
  expect(vi.mocked(deviceRequest).mock.calls[1]).toEqual(vi.mocked(deviceRequest).mock.calls[0]);
});

it('stops after one busy retry and propagates the reason', async () => {
  vi.useFakeTimers();
  const error = new ApiError(409, { code: 'checkpoint_busy', message: 'Harness is still in use' });
  vi.mocked(deviceRequest).mockRejectedValue(error);
  const result = expect(gitWrite('thread', 'checkpoints/restore', {})).rejects.toBe(error);
  await vi.advanceTimersByTimeAsync(750);
  await result;
  expect(deviceRequest).toHaveBeenCalledTimes(2);
});

it.each([
  ['checkpoints/restore', 409, 'conflict'],
  ['checkpoints/restore', 500, 'checkpoint_busy'],
  ['checkpoints/preview', 409, 'checkpoint_busy'],
  ['rewind', 409, 'checkpoint_busy'],
] as const)('does not retry %s for %s %s', async (action, status, code) => {
  const error = new ApiError(status, { code, message: 'refused' });
  vi.mocked(deviceRequest).mockRejectedValue(error);
  await expect(gitWrite('thread', action, {})).rejects.toBe(error);
  expect(deviceRequest).toHaveBeenCalledTimes(1);
});
