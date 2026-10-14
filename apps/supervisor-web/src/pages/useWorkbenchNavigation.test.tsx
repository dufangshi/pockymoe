import { browserStateKey } from '../lib/browserStateScope';
import { HostCapabilitiesProvider } from '../lib/hostCapabilities';
import type { ReactNode } from 'react';
import type { RuntimeConfigDto } from '@pockymoe/shared';
import { act, renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { ThreadDetailDto, ThreadDto } from '@pockymoe/shared';
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
