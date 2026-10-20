import { browserStateKey } from '../lib/browserStateScope';
import { HostCapabilitiesProvider } from '../lib/hostCapabilities';
import type { ReactNode } from 'react';
import type { RuntimeConfigDto } from '@pockymoe/shared';
import { act, renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { ThreadDetailDto, ThreadDto, WorkspaceDto } from '@pockymoe/shared';
import { useWorkbenchNavigation, workbenchThreadStatus } from './useWorkbenchNavigation';
import { ApiError, request, fetchRuntimeConfig, relayModeActive } from '../lib/api';
import { useScopedState } from './useScopedState';

vi.mock('../lib/api', async original => ({
  ...(await original<typeof import('../lib/api')>()),
  request: vi.fn(),
  fetchRuntimeConfig: vi.fn(),
  relayModeActive: vi.fn(() => true),
}));

describe('account thread navigation', () => {
  it('lists a workspace before any thread is selected and follows workspace switches', async () => {
    vi.mocked(request).mockImplementation(async url => {
      if (url === 'relay/account/workbench') return { threads: [], notifications: [] };
      throw new Error(`Unexpected URL: ${url}`);
    });
    const root = { id: 'root', workspaceId: 'app', title: 'Root', status: 'idle' } as ThreadDto;
    const worker = { ...root, id: 'worker', workspaceId: 'docs', parentThreadId: root.id, rootThreadId: root.id };
    const other = { ...root, id: 'other', workspaceId: 'docs' };
    const { result, rerender, unmount } = renderHook(({ workspace }) =>
      useWorkbenchNavigation(null, [root, worker, other], 'laptop', undefined, { workspace, sharesDeviceThreads: true }), {
        initialProps: { workspace: { id: 'app', label: 'App' } as WorkspaceDto },
      });
    await waitFor(() => expect(result.current.navigationReady).toBe(true));
    expect(result.current.currentKey).toBe('laptop:');
    expect(result.current.workspaceThreads.map(thread => thread.key)).toEqual(['laptop:root', 'laptop:worker']);
    expect(result.current.workspaceThreads[0]?.subtitle).toBe('App');
    rerender({ workspace: { id: 'docs', label: 'Docs' } as WorkspaceDto });
    expect(result.current.workspaceThreads.map(thread => thread.key)).toEqual(['laptop:other']);
    expect(result.current.currentKey).toBe('laptop:');
    unmount();
  });
  it('saves a shortcut order for the account and shows it before the relay answers', async () => {
    const reference = (threadId: string, favoriteOrder?: number) => ({
      deviceId: 'wsl', threadId, title: threadId.toUpperCase(), workspaceLabel: 'Research', favorite: true,
      visitedAt: '2026-10-16T00:00:00Z', ...(favoriteOrder === undefined ? {} : { favoriteOrder }),
    });
    let saved: unknown = null;
    vi.mocked(request).mockImplementation(async (url, init) => {
      if (url === 'relay/account/workbench') return { threads: [reference('a'), reference('b')], notifications: [] };
      if (url === 'relay/account/workbench/shortcuts') {
        saved = JSON.parse(String(init?.body));
        return { threads: [reference('a', 1), reference('b', 0)], notifications: [] };
      }
      return { thread: { id: String(url).split('/threads/')[1]?.split('?')[0], status: 'idle' } };
    });
    const { result, unmount } = renderHook(() => useWorkbenchNavigation(null, [], 'wsl'));
    await waitFor(() => expect(result.current.threads.map(thread => thread.key)).toEqual(['wsl:a', 'wsl:b']));
    expect(result.current.threads.every(thread => thread.shortcutOrder === undefined)).toBe(true);
    await act(async () => { result.current.onReorderShortcuts(['wsl:b', 'wsl:a']); });
    expect(saved).toEqual({ order: [{ deviceId: 'wsl', threadId: 'b' }, { deviceId: 'wsl', threadId: 'a' }] });
    await waitFor(() => expect(Object.fromEntries(result.current.threads.map(thread => [thread.key, thread.shortcutOrder]))).toEqual({ 'wsl:a': 1, 'wsl:b': 0 }));
    unmount();
  });


  it('loads complete sidebar families without selecting them, deduplicates visits and replaces polled children', async () => {
    const parent = { id: 'root', workspaceId: 'research', title: 'Reaxys', status: 'idle' } as ThreadDto;
    const children = Array.from({ length: 8 }, (_, index) => ({
      ...parent, id: `agent-${index}`, title: `Agent ${index}`, workspaceId: index === 7 ? 'other-workspace' : parent.workspaceId,
      parentThreadId: index === 7 ? 'agent-0' : parent.id, rootThreadId: parent.id,
    }));
    let family = [parent, ...children];
    let offline = false;
    let groupRequests = 0;
    const references = [parent, children[0]!].map(thread => ({ deviceId: 'wsl', threadId: thread.id, title: thread.title, workspaceLabel: 'Research', favorite: false }));
    vi.mocked(request).mockImplementation(async url => {
      if (url === 'relay/account/workbench') return { threads: references, notifications: [] };
      if (offline) throw new ApiError(503, { code: 'service_unavailable', message: 'Device offline' });
      if (String(url).includes('includeAgentThreads')) throw new ApiError(403, { code: 'forbidden', message: 'Thread-scoped access' });
      if (String(url).endsWith('/root/group')) { groupRequests++; return family; }
      const id = String(url).split('/threads/')[1]?.split('?')[0];
      return { thread: family.find(thread => thread.id === id) };
    });
    const mac = { thread: { ...parent, id: 'mac-root' }, workspace: { label: 'App' } } as ThreadDetailDto;
    const wsl = { thread: parent, workspace: { label: 'Research' } } as ThreadDetailDto;
    const { result, rerender, unmount } = renderHook(({ detail, threads, device }) =>
      useWorkbenchNavigation(detail, threads, device), {
        initialProps: { detail: mac, threads: [mac.thread], device: 'mac' },
      });
    const agents = () => result.current.threads.filter(thread => thread.rootKey === 'wsl:root');
    await waitFor(() => expect(agents()).toHaveLength(8));
    expect(groupRequests).toBe(1); // Root and previously visited child share one query.
    expect(agents().find(thread => thread.key === 'wsl:agent-7')).toMatchObject({ parentKey: 'wsl:agent-0' });
    rerender({ detail: wsl, threads: family, device: 'wsl' });
    expect(agents()).toHaveLength(8);
    await waitFor(() => expect(groupRequests).toBe(2));
    rerender({ detail: mac, threads: [mac.thread], device: 'mac' });
    expect(agents()).toHaveLength(8);
    await waitFor(() => expect(groupRequests).toBe(3));
    family = [parent, ...children.slice(0, 7)];
    act(() => document.dispatchEvent(new Event('visibilitychange')));
    await waitFor(() => expect(agents()).toHaveLength(7));
    offline = true;
    act(() => document.dispatchEvent(new Event('visibilitychange')));
    await waitFor(() => expect(result.current.threads.find(thread => thread.key === 'wsl:root')?.status).toBe('unknown'));
    expect(agents()).toHaveLength(7);
    // An unreachable device is left alone for a while instead of on every refresh.
    const before = vi.mocked(request).mock.calls.filter(([url]) => String(url).includes('relay/devices/wsl/')).length;
    act(() => document.dispatchEvent(new Event('visibilitychange')));
    await waitFor(() => expect(vi.mocked(request).mock.calls.filter(([url]) => url === 'relay/account/workbench').length).toBeGreaterThan(3));
    await new Promise(resolve => setTimeout(resolve, 50));
    expect(vi.mocked(request).mock.calls.filter(([url]) => String(url).includes('relay/devices/wsl/')).length).toBe(before);
    expect(result.current.threads.find(thread => thread.key === 'wsl:root')?.status).toBe('unknown');
    unmount();
    offline = false;
    const reloaded = renderHook(() => useWorkbenchNavigation(mac, [mac.thread], 'mac'));
    await waitFor(() => expect(reloaded.result.current.threads.filter(thread => thread.rootKey === 'wsl:root')).toHaveLength(7));
    reloaded.unmount();
  });

  it('fills recent families from every local workspace, independently of workspace tabs', async () => {
    const selected = { id: 'selected', workspaceId: 'app', title: 'App', status: 'idle' } as ThreadDto;
    const root = { ...selected, id: 'research-root', workspaceId: 'research' };
    const children = Array.from({ length: 8 }, (_, index) => ({ ...root, id: `research-${index}`, parentThreadId: root.id, rootThreadId: root.id }));
    vi.mocked(request).mockImplementation(async url => url === 'relay/account/workbench'
      ? { threads: [{ deviceId: 'wsl', threadId: root.id, title: root.title, favorite: false }], notifications: [] }
      : { thread: root });
    const detail = { thread: selected, workspace: { label: 'App' } } as ThreadDetailDto;
    const { result, unmount } = renderHook(() => useWorkbenchNavigation(detail, [selected, root, ...children], 'wsl'));
    await waitFor(() => expect(result.current.navigationReady).toBe(true));
    expect(result.current.workspaceThreads.map(thread => thread.key)).toEqual(['wsl:selected']);
    expect(result.current.threads.filter(thread => thread.rootKey === 'wsl:research-root')).toHaveLength(8);
    unmount();
  });

  it('batches complete lists per device and keeps same-ID families separate', async () => {
    const refs = ['mac', 'wsl'].flatMap(deviceId => ['root', 'child'].map(threadId => ({ deviceId, threadId, title: threadId, favorite: false })));
    const root = { id: 'root', workspaceId: 'app', title: 'Root', status: 'idle' } as ThreadDto;
    const child = { ...root, id: 'child', parentThreadId: 'root', rootThreadId: 'root' };
    const calls: string[] = [];
    vi.mocked(request).mockImplementation(async url => {
      calls.push(String(url));
      if (url === 'relay/account/workbench') return { threads: refs, notifications: [] };
      if (String(url).includes('/mac/api/threads?')) return [root, child];
      if (String(url).includes('/wsl/api/threads?')) return [root, child, { ...child, id: 'another-child' }];
      throw new Error(`Unexpected URL: ${url}`);
    });
    const { result, unmount } = renderHook(() => useWorkbenchNavigation(null, [], 'elsewhere'));
    await waitFor(() => expect(result.current.threads).toHaveLength(5));
    expect(result.current.threads.filter(thread => thread.rootKey === 'mac:root')).toHaveLength(1);
    expect(result.current.threads.filter(thread => thread.rootKey === 'wsl:root')).toHaveLength(2);
    expect(calls).toEqual([
      'relay/account/workbench',
      'relay/devices/mac/api/threads?includeAgentThreads=true',
      'relay/devices/wsl/api/threads?includeAgentThreads=true',
    ]);
    unmount();
  });

  it('lists running threads from every device it reads, opened before or not', async () => {
    const refs = [
      { deviceId: 'mac', threadId: 'root', title: 'Root', favorite: false, workspaceId: 'app', workspaceLabel: 'app', deviceName: 'Mac' },
      // Opened at 10:00, answered at 11:00: unread.
      { deviceId: 'mac', threadId: 'done', title: 'Done', favorite: false, workspaceId: 'app', workspaceLabel: 'app', deviceName: 'Mac', readCompletedAt: '2026-10-16T10:00:00Z' },
    ];
    const root = { id: 'root', workspaceId: 'app', title: 'Root', status: 'idle' } as ThreadDto;
    const automation = { ...root, id: 'nightly', title: 'Nightly check', status: 'running' } as ThreadDto;
    const worker = { ...root, id: 'worker', title: 'Worker', status: 'running', parentThreadId: 'root', rootThreadId: 'root' } as ThreadDto;
    vi.mocked(request).mockImplementation(async url => {
      if (url === 'relay/account/workbench') return { threads: refs, notifications: [] };
      if (String(url).includes('/mac/api/threads?')) return [root, automation, worker,
        { ...root, id: 'done', title: 'Done', lastTurnCompletedAt: '2026-10-16T11:00:00Z' },
        // Finished but never opened: not unread.
        { ...root, id: 'old', title: 'Old', lastTurnCompletedAt: '2026-10-01T11:00:00Z' }];
      throw new Error(`Unexpected URL: ${url}`);
    });
    const own = { ...root, id: 'here', title: 'Here', status: 'running' } as ThreadDto;
    const { result, unmount } = renderHook(() => useWorkbenchNavigation(null, [own, { ...root, id: 'quiet' }], 'wsl'));
    await waitFor(() => expect(result.current.activeThreads.map(thread => `${thread.key}:${thread.status}`))
      .toEqual(['wsl:here:running', 'mac:nightly:running', 'mac:worker:running', 'mac:done:unread', 'mac:root:idle']));
    expect(result.current.activeThreads[1]).toMatchObject({ title: 'Nightly check', subtitle: 'Mac · app', status: 'running', href: '/devices/mac/threads/nightly' });
    expect(result.current.activeThreads[2]).toMatchObject({ rootKey: 'mac:root' });
    unmount();
  });

  it('reuses the page thread list for its own device and fetches only other devices', async () => {
    const refs = ['mac', 'wsl'].flatMap(deviceId => ['root', 'child'].map(threadId => ({ deviceId, threadId, title: threadId, favorite: false })));
    const root = { id: 'root', workspaceId: 'app', title: 'Root', status: 'idle' } as ThreadDto;
    const child = { ...root, id: 'child', parentThreadId: 'root', rootThreadId: 'root' };
    const calls: string[] = [];
    vi.mocked(request).mockImplementation(async url => {
      calls.push(String(url));
      if (url === 'relay/account/workbench') return { threads: refs, notifications: [] };
      if (String(url).includes('/mac/api/threads?')) return [root, child];
      throw new Error(`Unexpected URL: ${url}`);
    });
    const { result, rerender, unmount } = renderHook(({ threads }) =>
      useWorkbenchNavigation(null, threads, 'wsl', undefined, { sharesDeviceThreads: true }),
    { initialProps: { threads: [] as ThreadDto[] } });
    // Before the page list loads, its device's references wait instead of
    // fetching one summary each.
    await waitFor(() => expect(result.current.navigationReady).toBe(true));
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls).toEqual(['relay/account/workbench', 'relay/devices/mac/api/threads?includeAgentThreads=true']);
    rerender({ threads: [root, { ...child, status: 'running' }] });
    await waitFor(() => expect(result.current.threads.find(thread => thread.key === 'wsl:child')?.status).toBe('running'));
    expect(calls.some(url => url.includes('/wsl/'))).toBe(false);
    unmount();
  });

  it('retains account navigation during device switches without recording the previous device transcript', async () => {
    const focus = vi.spyOn(document, 'hasFocus').mockReturnValue(true);
    const refs = ['mac', 'wsl'].map(deviceId => ({ deviceId, threadId: `${deviceId}-thread`, title: deviceId, workspaceLabel: 'App', favorite: false, visitedAt: '' }));
    const writes: { deviceId: string; threadId: string }[] = [];
    vi.mocked(request).mockImplementation(async (url, init) => {
      if (url === 'relay/account/workbench') {
        if (init?.method === 'POST') writes.push(JSON.parse(String(init.body)));
        return { threads: refs, notifications: [] };
      }
      return { thread: { status: 'idle' } };
    });
    const mac = { thread: { id: 'mac-thread', workspaceId: 'app', title: 'Mac', status: 'idle' }, workspace: { label: 'App', absPath: '/mac/app' } } as ThreadDetailDto;
    const { result, rerender, unmount } = renderHook(({ device }) => {
      const [detail, setDetail] = useScopedState<ThreadDetailDto | null>(device, null);
      return { setDetail, nav: useWorkbenchNavigation(detail, detail ? [detail.thread] : [], device, `${device}-thread`) };
    }, { initialProps: { device: 'mac' } });
    act(() => result.current.setDetail(mac));
    await waitFor(() => expect(writes).toContainEqual(expect.objectContaining({ deviceId: 'mac', threadId: 'mac-thread' })));
    const oldSetter = result.current.setDetail;
    rerender({ device: 'wsl' });
    expect(result.current.nav.threads).toHaveLength(2);
    expect(result.current.nav.navigationReady).toBe(true);
    expect(result.current.nav.workspaceThreads[0]?.href).toBe('/devices/mac/threads/mac-thread');
    act(() => window.dispatchEvent(new Event('focus')));
    act(() => oldSetter(mac));
    expect(writes.some(w => w.deviceId === 'wsl' && w.threadId === 'mac-thread')).toBe(false);
    act(() => result.current.setDetail({ ...mac, thread: { ...mac.thread, id: 'wsl-thread', title: 'WSL' } }));
    await waitFor(() => expect(writes).toContainEqual(expect.objectContaining({ deviceId: 'wsl', threadId: 'wsl-thread' })));
    expect(result.current.nav.workspaceThreads[0]?.key).toBe('wsl:wsl-thread');
    unmount(); focus.mockRestore();
  });
  it('removes only confirmed missing thread references, retaining offline and denied devices', async () => {
    const refs = ['missing', 'offline', 'denied'].map(deviceId => ({ deviceId, threadId: 'old-thread', title: deviceId }));
    const deletes: unknown[] = [];
    vi.mocked(request).mockImplementation(async (url, init) => {
      if (url === 'relay/account/workbench') {
        if (init?.method === 'DELETE') deletes.push(JSON.parse(String(init.body)));
        return { threads: refs, notifications: [] };
      }
      if (String(url).includes('/missing/')) throw new ApiError(404, { code: 'not_found', message: 'Thread not found' });
      if (String(url).includes('/offline/')) throw new ApiError(503, { code: 'unavailable', message: 'Device offline' } as never);
      throw new ApiError(404, { code: 'not_found', message: 'Device not found' });
    });
    const { result, unmount } = renderHook(() => useWorkbenchNavigation(null, [], 'missing'));
    await waitFor(() => expect(deletes).toEqual([{ deviceId: 'missing', threadId: 'old-thread' }]));
    await waitFor(() => expect(result.current.threads.map(t => t.key)).toEqual(['offline:old-thread', 'denied:old-thread']));
    unmount();
  });
  it('uses every current-workspace thread for tabs, independently of account recents and active tab', async () => {
    vi.mocked(request).mockResolvedValue({ threads: [{ deviceId: 'wsl', threadId: 'elsewhere', title: 'Recent elsewhere', favorite: false }], notifications: [] });
    const detail = { thread: { id: 'second', workspaceId: 'app', title: 'Second', status: 'running', createdAt: '2026-09-02' }, workspace: { label: 'App' } } as ThreadDetailDto;
    const threads = [detail.thread, { ...detail.thread, id: 'first', createdAt: '2026-09-01' }, { ...detail.thread, id: 'other', workspaceId: 'another' }];
    const { result, rerender, unmount } = renderHook(({ current }) => useWorkbenchNavigation(current, threads, 'wsl'), { initialProps: { current: detail } });
    await waitFor(() => expect(result.current.navigationReady).toBe(true));
    expect(result.current.workspaceThreads.map(t => t.key)).toEqual(['wsl:first', 'wsl:second']);
    expect(result.current.threads.map(t => t.key)).toContain('wsl:elsewhere');
    rerender({ current: { ...detail, thread: threads[1]! } });
    expect(result.current.workspaceThreads.map(t => t.key)).toEqual(['wsl:first', 'wsl:second']);
    unmount();
  });
  it('keeps another workspace\'s thread family out of the tabs, even when one of its agent threads works here', async () => {
    vi.mocked(request).mockResolvedValue({ threads: [], notifications: [] });
    const detail = { thread: { id: 'here', workspaceId: 'elagente', title: 'Here', status: 'idle', createdAt: '2026-10-01' }, workspace: { label: 'ElAgente' } } as ThreadDetailDto;
    const slack = { id: 'slack', workspaceId: 'slack', title: 'slack', status: 'idle', createdAt: '2026-10-02' } as ThreadDto;
    const threads = [detail.thread, slack,
      { ...slack, id: 'slack-worker', parentThreadId: 'slack', rootThreadId: 'slack', workspaceId: 'elagente', createdAt: '2026-10-03' } as ThreadDto,
      { ...slack, id: 'slack-helper', parentThreadId: 'slack', rootThreadId: 'slack', createdAt: '2026-10-04' } as ThreadDto,
      // An agent thread a conversation here started in another workspace stays in its family.
      { ...slack, id: 'here-elsewhere', parentThreadId: 'here', rootThreadId: 'here', createdAt: '2026-10-05' } as ThreadDto];
    const { result, unmount } = renderHook(() => useWorkbenchNavigation(detail, threads, 'wsl'));
    await waitFor(() => expect(result.current.navigationReady).toBe(true));
    // slack-worker works here but belongs to slack's conversation.
    expect(result.current.workspaceThreads.map(t => t.key)).toEqual(['wsl:here', 'wsl:here-elsewhere']);
    unmount();
    // Open, it shows inside its conversation.
    const open = { ...detail, thread: threads[2]! } as ThreadDetailDto;
    const opened = renderHook(() => useWorkbenchNavigation(open, threads, 'wsl'));
    await waitFor(() => expect(opened.result.current.navigationReady).toBe(true));
    expect(opened.result.current.workspaceThreads.map(t => t.key)).toEqual(['wsl:here', 'wsl:slack', 'wsl:slack-worker', 'wsl:here-elsewhere']);
    expect(opened.result.current.workspaceThreads[2]!.rootKey).toBe('wsl:slack');
    opened.unmount();
  });
  it('labels each tab with its own workspace, read from the device when nothing else names it', async () => {
    const calls: string[] = [];
    vi.mocked(request).mockImplementation(async url => {
      calls.push(String(url));
      if (url === 'relay/devices/wsl/api/workspaces') return [{ id: 'site', label: 'Site' }, { id: 'docs', label: 'Docs' }];
      return { threads: [], notifications: [] };
    });
    const root = { id: 'root', workspaceId: 'site', title: 'Rebuild', status: 'idle', createdAt: '2026-10-01' } as ThreadDto;
    const child = { ...root, id: 'child', workspaceId: 'docs', title: 'Update docs', parentThreadId: 'root', rootThreadId: 'root', createdAt: '2026-10-02' } as ThreadDto;
    const detail = { thread: child, workspace: { id: 'docs', label: 'Docs' } } as ThreadDetailDto;
    const { result, unmount } = renderHook(() => useWorkbenchNavigation(detail, [root, child], 'wsl'));
    // The open child's workspace is known at once; its conversation's is read once.
    expect(result.current.workspaceThreads.map(t => [t.key, t.subtitle])).toEqual([['wsl:root', ''], ['wsl:child', 'Docs']]);
    await waitFor(() => expect(result.current.workspaceThreads.map(t => t.subtitle)).toEqual(['Site', 'Docs']));
    expect(result.current.workspaceLabel('site')).toBe('Site');
    expect(calls.filter(url => url.endsWith('/api/workspaces'))).toHaveLength(1);
    unmount();
  });
  it('brings the conversation of a running agent thread along, for grouping', async () => {
    vi.mocked(request).mockResolvedValue({ threads: [], notifications: [] });
    const detail = { thread: { id: 'here', workspaceId: 'w', title: 'Here', status: 'idle', createdAt: '2026-10-01' }, workspace: { label: 'W' } } as ThreadDetailDto;
    const root = { id: 'root', workspaceId: 'w2', title: 'Root', status: 'idle', createdAt: '2026-10-02' } as ThreadDto;
    const worker = { ...root, id: 'worker', title: 'Worker', status: 'running', parentThreadId: 'root', rootThreadId: 'root', activeTurnId: 't' } as ThreadDto;
    const { result, unmount } = renderHook(() => useWorkbenchNavigation(detail, [detail.thread, root, worker], 'wsl'));
    await waitFor(() => expect(result.current.navigationReady).toBe(true));
    const active = result.current.activeThreads;
    expect(active.find(t => t.key === 'wsl:worker')?.status).toBe('running');
    expect(active.find(t => t.key === 'wsl:root')).toMatchObject({ title: 'Root', status: 'idle' });
    unmount();
  });
  it('shows a rename or deletion made from the sidebar before the thread list catches up', async () => {
    vi.mocked(request).mockResolvedValue({ threads: [], notifications: [] });
    const detail = { thread: { id: 'here', workspaceId: 'w', title: 'Here', status: 'idle', createdAt: '2026-10-01' }, workspace: { label: 'W' } } as ThreadDetailDto;
    const threads = [detail.thread, { ...detail.thread, id: 'old', title: 'Old name', createdAt: '2026-10-02' } as ThreadDto,
      { ...detail.thread, id: 'gone', title: 'Gone', createdAt: '2026-10-03' } as ThreadDto];
    const { result, rerender, unmount } = renderHook(({ list }) => useWorkbenchNavigation(detail, list, 'wsl'), { initialProps: { list: threads } });
    await waitFor(() => expect(result.current.navigationReady).toBe(true));
    await act(async () => { await result.current.onThreadRenamed('wsl:old', 'New name'); await result.current.onThreadRemoved('wsl:gone'); });
    expect(result.current.workspaceThreads.map(t => t.title)).toEqual(['Here', 'New name']);
    // A later rename elsewhere shows once the list has it.
    rerender({ list: [detail.thread, { ...threads[1]!, title: 'Renamed elsewhere' }] });
    expect(result.current.workspaceThreads.map(t => t.title)).toEqual(['Here', 'Renamed elsewhere']);
    unmount();
  });
  it('loads notification titles and reply previews using the notification device, even without a saved shortcut', async () => {
    const completed = '2026-09-19T14:00:00Z';
    const snapshot = { threads: [], notifications: [{ id: 'event', title: '1 completed', href: '/devices/mac/threads/review', occurredAt: completed }] };
    vi.mocked(request).mockImplementation(async url => {
      if (url === 'relay/account/workbench') return snapshot;
      if (url === 'relay/devices/mac/api/threads/review?view=summary&limit=10&compact=1') return {
        thread: { title: 'Fix the image preview' }, turns: [{ status: 'completed', completedAt: completed, items: [{ kind: 'agentMessage', text: 'The image now loads correctly. ' + 'Details '.repeat(50) }] }],
      };
      throw new Error(`Unexpected URL: ${url}`);
    });
    const { result, unmount } = renderHook(() => useWorkbenchNavigation(null, [], 'wsl'));
    await waitFor(() => expect(result.current.notifications).toHaveLength(1));
    act(() => result.current.onReadNotifications());
    await waitFor(() => expect(result.current.notifications[0]?.title).toBe('Fix the image preview · Completed'));
    expect(result.current.notifications[0]?.summary).toMatch(/^The image now loads correctly/);
    expect(Array.from(result.current.notifications[0]!.summary!)).toHaveLength(181);
    expect(result.current.notifications[0]?.summary).toMatch(/…$/);
    unmount();
  });
  it('passes device-scoped lineage to tabs and preserves it while selecting a child', async () => {
    vi.mocked(request).mockResolvedValue({threads: [], notifications: []});
    const parent = {id:'root',workspaceId:'app',title:'Root',status:'idle',createdAt:'2026-09-01'} as ThreadDto;
    const child = {...parent,id:'child',title:'Child',parentThreadId:'root',rootThreadId:'root',createdAt:'2026-09-02'};
    const detail = {thread:child, workspace:{label:'App'}} as ThreadDetailDto;
    const {result,unmount} = renderHook(() => useWorkbenchNavigation(detail,[parent,child],'wsl'));
    expect(result.current.workspaceThreads.find(t=>t.key==='wsl:child')).toMatchObject({parentKey:'wsl:root',rootKey:'wsl:root'});
    unmount();
  });
  it('distinguishes running, unread completion, read idle, failure and unavailable states', () => {
    const completed = '2026-09-19T14:00:00Z';
    const activity = { status: 'idle', lastTurnCompletedAt: completed };
    expect(workbenchThreadStatus({ ...activity, status: 'running' })).toBe('running');
    expect(workbenchThreadStatus(activity)).toBe('unread');
    expect(workbenchThreadStatus(activity, completed)).toBe('idle');
    expect(workbenchThreadStatus({ ...activity, lastTurnCompletedAt: '2026-09-19T15:00:00Z' }, completed)).toBe('unread');
    expect(workbenchThreadStatus({ ...activity, status: 'failed' }, completed)).toBe('failed');
    expect(workbenchThreadStatus({ ...activity, status: 'system_error' })).toBe('failed');
    expect(workbenchThreadStatus({ status: 'idle' })).toBe('idle');
    expect(workbenchThreadStatus({ status: 'not_loaded' })).toBe('unknown');
    expect(workbenchThreadStatus(undefined)).toBe('unknown');
  });

  it('acknowledges a completed turn only while the thread is focused, and never through a favorite write', async () => {
    const focus = vi.spyOn(document, 'hasFocus').mockReturnValue(false);
    const completion = '2026-09-19T14:00:00Z';
    let ref = { deviceId: 'mac', threadId: 'read-test', title: 'Read state', workspaceLabel: 'App', favorite: false, visitedAt: '', readCompletedAt: null as string | null };
    vi.mocked(request).mockClear();
    vi.mocked(request).mockImplementation(async (_url, init) => {
      if (init?.method === 'POST') ref = { ...ref, ...JSON.parse(String(init.body)) };
      return { threads: [ref], notifications: [] };
    });
    const detail = { thread: { id: 'read-test', title: 'Read state', status: 'idle', lastTurnCompletedAt: completion }, workspace: { label: 'App' } } as ThreadDetailDto;
    const { result, unmount } = renderHook(() => useWorkbenchNavigation(detail, [detail.thread], 'mac'));
    await waitFor(() => expect(result.current.threads[0]?.status).toBe('unread'));
    act(() => result.current.onToggleFavorite());
    await waitFor(() => expect(result.current.favorite).toBe(true));
    expect(ref.readCompletedAt).toBeNull();
    expect(result.current.threads[0]?.status).toBe('unread');
    focus.mockReturnValue(true);
    act(() => window.dispatchEvent(new Event('focus')));
    await waitFor(() => expect(result.current.threads[0]?.status).toBe('idle'));
    expect(ref.readCompletedAt).toBe(completion);
    unmount();
    focus.mockRestore();
  });
  it('keeps same-ID threads on different devices separate and monitors shared threads with explicit URLs', async () => {
    const references = [
      {
        deviceId: 'mac',
        threadId: 'same-id',
        title: 'Mac review',
        workspaceLabel: 'App',
        favorite: false,
        visitedAt: '2026-09-19',
      },
      {
        deviceId: 'wsl',
        threadId: 'same-id',
        title: 'WSL review',
        workspaceLabel: 'Server',
        favorite: true,
        visitedAt: '2026-09-18',
      },
      {
        deviceId: 'shared-device',
        threadId: 'shared-thread',
        title: 'Shared review',
        workspaceLabel: 'Team',
        favorite: true,
        visitedAt: '2026-09-18',
      },
    ];
    vi.mocked(request).mockImplementation(async (url, init) => {
      if (url === 'relay/account/workbench') {
        if (init?.method === 'POST') {
          const visit = JSON.parse(String(init.body));
          if (visit.favorite !== undefined)
            references.find(
              (r) =>
                r.deviceId === visit.deviceId && r.threadId === visit.threadId,
            )!.favorite = visit.favorite;
        }
        return { threads: structuredClone(references), notifications: [] };
      }
      if (String(url).includes('/shared-device/'))
        return { thread: { status: 'running' } };
      return { thread: { status: 'idle' } };
    });
    const detail = {
      thread: {
        id: 'same-id',
        workspaceId: 'app',
        title: 'Mac review',
        status: 'idle',
      },
      workspace: { label: 'App' },
    } as ThreadDetailDto;
    const { result } = renderHook(() =>
      useWorkbenchNavigation(detail, [detail.thread], 'mac'),
    );
    await waitFor(() => expect(result.current.threads).toHaveLength(3));
    await waitFor(() =>
      expect(
        result.current.threads.find(
          (t) => t.key === 'shared-device:shared-thread',
        )?.status,
      ).toBe('running'),
    );
    expect(
      result.current.threads.find((t) => t.key === 'wsl:same-id')?.href,
    ).toBe('/devices/wsl/threads/same-id');
    expect(result.current.favorite).toBe(false);
    act(() => result.current.onToggleFavorite());
    await waitFor(() => expect(result.current.favorite).toBe(true));
    expect(request).toHaveBeenCalledWith(
      'relay/devices/shared-device/api/threads/shared-thread?view=summary&limit=1&compact=1',
      expect.anything(),
    );
    const writes = vi
      .mocked(request)
      .mock.calls.filter(([, init]) => init?.method === 'POST')
      .map(([, init]) => JSON.parse(String(init?.body)));
    expect(writes.at(-1)).toMatchObject({
      deviceId: 'mac',
      threadId: 'same-id',
      favorite: true,
    });
    expect(references.find((t) => t.deviceId === 'wsl')?.favorite).toBe(true);
    await act(async () => result.current.onToggleThreadFavorite('wsl:same-id'));
    expect(references.find((t) => t.deviceId === 'wsl')?.favorite).toBe(false);
    expect(references.find((t) => t.deviceId === 'mac')?.favorite).toBe(true);
    expect(request).toHaveBeenLastCalledWith('relay/account/workbench', expect.objectContaining({
      method: 'POST', body: expect.stringContaining('"deviceId":"wsl"'),
    }));
  });
});

it('opens saved threads from every bound Agent on this hub and silently drops forbidden/missing references', async () => {
  localStorage.clear();
  // A Treer hub serves its Web UI locally, not through a relay.
  vi.mocked(relayModeActive).mockReturnValue(false);
  const scope = { kind: 'treerAgent', boundThreadId: 'root', hubId: 'hub-one' } as NonNullable<RuntimeConfigDto['hostScope']>;
  const root = { id: 'root', workspaceId: 'project', title: 'Root', status: 'idle' } as ThreadDto;
  const child = { ...root, id: 'child', parentThreadId: 'root', rootThreadId: 'root', workspaceId: 'child-project' };
  const other = { ...root, id: '333', title: 'Other bound Agent' };
  const key = browserStateKey('remote-codex.local-workbench.v1', scope);
  const stored = { threads: [root,child,other,{...root,id:'forbidden'},{...root,id:'missing'}].map(thread => ({
    deviceId: null, threadId: thread.id, title: thread.title, workspaceLabel: 'Project', favorite: true,
  })), notifications: [] };
  localStorage.setItem(key, JSON.stringify(stored));
  localStorage.setItem('remote-codex.local-workbench.v1', JSON.stringify({ threads: [{ deviceId:null,threadId:'leaked-old-state' }],notifications:[] }));
  vi.mocked(fetchRuntimeConfig).mockResolvedValue({ hostScope: scope } as RuntimeConfigDto);
  vi.mocked(request).mockImplementation(async path => {
    if (String(path).includes('forbidden')) throw new ApiError(403, { code:'thread_scope_forbidden',message:'Denied' });
    if (String(path).includes('missing')) throw new ApiError(404, { code:'not_found',message:'Missing' });
    throw new Error(`Unexpected request ${path}`);
  });
  const detail = { thread: child, workspace: { label: 'Child project' } } as ThreadDetailDto;
  const wrapper = ({ children }: { children: ReactNode }) => <HostCapabilitiesProvider scope="local">{children}</HostCapabilitiesProvider>;
  const { result, unmount } = renderHook(() => useWorkbenchNavigation(detail, [root, child, other], null), { wrapper });
  await waitFor(() => expect(result.current?.navigationReady).toBe(true));
  expect(result.current.threads.map(thread => thread.key).sort()).toEqual(['local:333','local:child','local:root']);
  expect(result.current.threads.find(thread => thread.key === 'local:333')).toMatchObject({href:'/threads/333',favorite:true});
  expect(JSON.parse(localStorage.getItem(key)!).threads.map((r: {threadId:string}) => r.threadId)).toEqual(['root','child','333']);
  expect(result.current.error).toBeNull();
  unmount();
  const otherScope = { ...scope, hubId:'different-hub',boundThreadId:'other-root' };
  const otherRoot = { ...root,id:'other-root' };
  localStorage.setItem(browserStateKey('remote-codex.local-workbench.v1',otherScope), JSON.stringify({threads:[{
    deviceId:null,threadId:otherRoot.id,title:'Other hub',workspaceLabel:'Other hub',favorite:true,
  }],notifications:[]}));
  vi.mocked(fetchRuntimeConfig).mockResolvedValue({hostScope:otherScope} as RuntimeConfigDto);
  const otherHub = renderHook(() => useWorkbenchNavigation(null,[otherRoot],null),{wrapper});
  await waitFor(() => expect(otherHub.result.current?.navigationReady).toBe(true));
  expect(otherHub.result.current.threads.map(thread => thread.key)).toEqual(['local:other-root']);
  expect(otherHub.result.current.threads[0]?.favorite).toBe(true);
  otherHub.unmount(); localStorage.clear();
  vi.mocked(relayModeActive).mockReturnValue(true);
});

it('isolates navigation by hub and mount without importing legacy shortcuts', () => {
  const scope = { kind:'treerAgent',boundThreadId:'x',hubId:'hub-a' } as NonNullable<RuntimeConfigDto['hostScope']>;
  const key = (hubId: string, mount: string) => browserStateKey('remote-codex.local-workbench.v1',{...scope,hubId},mount);
  expect(new Set([key('hub-a','/agent-a/'),key('hub-a','/agent-b/'),key('hub-b','/agent-a/')]).size).toBe(3);
  expect(key('hub-a','/agent-a/')).not.toBe('remote-codex.local-workbench.v1');
  expect(browserStateKey('test',null)).toBe('test');
});

describe('thread switch', () => {
  it('keeps the workspace tabs while the switched-to thread is still loading', async () => {
    vi.mocked(request).mockImplementation(async url => {
      if (url === 'relay/account/workbench') return { threads: [], notifications: [] };
      return { thread: { id: String(url).split('/threads/')[1]?.split('?')[0], status: 'idle' } };
    });
    const thread = (id: string, createdAt: string) => ({ id, workspaceId: 'research', title: id, status: 'idle', createdAt }) as ThreadDto;
    const list = [thread('a', '2026-10-01'), thread('b', '2026-10-02'), thread('c', '2026-10-03')];
    // The page remounts on a switch: no detail yet, only the route and the device's list.
    const { result, unmount } = renderHook(() => useWorkbenchNavigation(null, list, 'wsl', 'b'));
    expect(result.current.workspaceThreads.map(tab => tab.key)).toEqual(['wsl:a', 'wsl:b', 'wsl:c']);
    expect(result.current.currentKey).toBe('wsl:b');
    unmount();
  });
});
