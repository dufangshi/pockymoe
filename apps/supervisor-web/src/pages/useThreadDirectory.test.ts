import { renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ThreadDetailDto, ThreadDto } from '@pockymoe/shared';
import { ApiError, fetchThreadDetail } from '../lib/api';
import { useThreadDirectory } from './useThreadDirectory';

vi.mock('../lib/api', async original => ({
  ...(await original<typeof import('../lib/api')>()),
  fetchThreadDetail: vi.fn(),
}));

const root = { id: 'root', workspaceId: 'site', title: 'Rebuild the home page' } as ThreadDto;
const child = { id: 'child', workspaceId: 'docs', title: 'Update docs', parentThreadId: 'root', rootThreadId: 'root' } as ThreadDto;
const detail = { thread: child, workspace: { id: 'docs', label: 'Docs' } } as ThreadDetailDto;
const labels = (id: string) => ({ site: 'Site' } as Record<string, string>)[id];

describe('useThreadDirectory', () => {
  beforeEach(() => { vi.mocked(fetchThreadDetail).mockReset(); });

  it('names listed threads with their workspaces and keeps unknown ones unknown', () => {
    const { result, rerender } = renderHook(({ threads }) => useThreadDirectory({ device: 'local', detail, threads, listSettled: true, workspaceLabel: labels }), { initialProps: { threads: [root, child] } });
    expect(result.current('root')).toEqual({ title: 'Rebuild the home page', workspaceId: 'site', workspaceLabel: 'Site' });
    expect(result.current('child')).toEqual({ title: 'Update docs', workspaceId: 'docs', workspaceLabel: 'Docs' });
    expect(result.current('stranger')).toBeUndefined();
    // A refreshed list with the same names keeps the function, so the transcript stays put.
    const first = result.current;
    rerender({ threads: [{ ...root }, { ...child }] });
    expect(result.current).toBe(first);
    rerender({ threads: [{ ...root, title: 'Renamed' }, child] });
    expect(result.current).not.toBe(first);
    expect(result.current('root')?.title).toBe('Renamed');
    expect(fetchThreadDetail).not.toHaveBeenCalled();
  });

  it('reads a parent missing from the list once the list has settled', async () => {
    vi.mocked(fetchThreadDetail).mockResolvedValue({ thread: root, workspace: { id: 'site', label: 'Site' } } as ThreadDetailDto);
    const { result, rerender } = renderHook(({ settled }) => useThreadDirectory({ device: 'local', detail, threads: [child], listSettled: settled, workspaceLabel: () => undefined }), { initialProps: { settled: false } });
    expect(result.current('root')).toBeUndefined();
    expect(fetchThreadDetail).not.toHaveBeenCalled();
    rerender({ settled: true });
    await waitFor(() => expect(result.current('root')).toEqual({ title: 'Rebuild the home page', workspaceId: 'site', workspaceLabel: 'Site' }));
    expect(fetchThreadDetail).toHaveBeenCalledExactlyOnceWith('root', { limit: 1 });
  });

  it('marks a parent the device refuses as unavailable', async () => {
    const refusal = new ApiError(403, { error: 'forbidden', code: 'thread_scope_forbidden' } as never);
    vi.mocked(fetchThreadDetail).mockImplementation(() => Promise.reject(refusal));
    const { result } = renderHook(() => useThreadDirectory({ device: 'mac', detail, threads: [child], listSettled: true, workspaceLabel: () => undefined }));
    await waitFor(() => expect(result.current('root')).toBeNull());
    expect(fetchThreadDetail).toHaveBeenCalledTimes(1);
  });
});
