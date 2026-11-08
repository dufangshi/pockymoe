import { expect, it, vi } from 'vitest';
import { deviceRequest } from './api';
import { fetchThreadGitCommits, fetchThreadGitDiff, fetchThreadGitStatus, fetchThreadWorkingContext } from './gitApi';
vi.mock('./api', () => ({ deviceRequest: vi.fn().mockResolvedValue({}) }));

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
