import { describe, expect, it, vi } from 'vitest';
import type { DesktopIndex, DesktopThread } from './data/types';
import { buildActivitySnapshot, focusedKeyFromPath, handleHostCommand, type CommandDependencies } from './activity';
const thread = (id: string, fields: Partial<DesktopThread> = {}) => ({ key: `d:${id}`, deviceId: 'd', id, workspaceId: 'w', title: '**Test**', provider: 'codex', status: 'idle', isPinned: false, updatedAt: '2026-11-01T00:00:00Z', createdAt: '', lastTurnStartedAt: null, lastTurnCompletedAt: null, activeTurnId: null, lastViewedAt: null, requests: [], resultSummary: '<b>done</b>\nsecond line', ...fields }) as DesktopThread;
const index = { accountId: 'a', devices: [{ id: 'd', name: 'Mac' }], workspaces: [{ id: 'w', deviceId: 'd', label: 'Project' }], threads: [] } as unknown as DesktopIndex;
describe('desktop host activity', () => {
  it('includes only actionable activity, plain-text summaries and focused route', () => {
    const now = Date.parse('2026-11-01T12:00:00Z');
    const snapshot = buildActivitySnapshot({ ...index, threads: [thread('idle'), thread('run', { status: 'running' }), thread('complete', { lastTurnCompletedAt: '2026-11-01T11:00:00Z' }), thread('old', { status: 'failed', lastTurnCompletedAt: '2026-10-01T11:00:00Z' }), thread('request', { requests: [{ id: 'r', kind: 'input', summary: '*'.repeat(200) + 'answer' }] })] }, 'd:run', true, now);
    expect(snapshot.items.map(row => row.state)).toEqual(['completed', 'needsInput', 'running']);
    expect(snapshot.items[0]?.resultSummary).toBe('done'); expect(snapshot.items[0]?.title).toBe('Test');
    expect(snapshot.items[1]?.request?.summary.length).toBeLessThanOrEqual(140);
    expect(snapshot.focusedThreadKey).toBe('d:run'); expect(buildActivitySnapshot(index, 'd:run', false).focusedThreadKey).toBeNull();
  });
  it('parses routes without treating new-thread as a focused thread', () => {
    expect(focusedKeyFromPath('/devices/d/threads/t')).toBe('d:t'); expect(focusedKeyFromPath('/devices/d/threads/new')).toBeNull();
  });
  const dependencies = () => ({ store: { markRead: vi.fn(), patchThread: vi.fn(), loadDetail: vi.fn() }, request: vi.fn(async () => ({})), navigate: vi.fn(), openPalette: vi.fn(), newThread: vi.fn() }) as unknown as CommandDependencies;
  it('handles navigation, read, palette, new thread and devices with results', async () => {
    const deps = dependencies();
    for (const command of [{ id: '1', type: 'navigate', path: '/devices/d/threads/t' }, { id: '2', type: 'markRead', key: 'd:t' }, { id: '3', type: 'openPalette' }, { id: '4', type: 'newThread' }, { id: '5', type: 'openDevices' }] as const) expect(await handleHostCommand(command, deps)).toEqual({ ok: true });
    expect(deps.store.markRead).toHaveBeenCalledWith('d:t'); expect(deps.navigate).toHaveBeenLastCalledWith('/desktop/devices');
    expect((await handleHostCommand({ id: '6', type: 'navigate', path: '//outside' }, deps)).ok).toBe(false);
  });
  it('sends replies on the command device and reports API failures', async () => {
    const deps = dependencies();
    expect(await handleHostCommand({ id: 'reply', type: 'reply', deviceId: 'b', threadId: 't', text: 'hello' }, deps)).toEqual({ ok: true });
    expect(deps.request).toHaveBeenCalledWith('b', '/api/threads/t/prompt', { method: 'POST', body: JSON.stringify({ prompt: 'hello', clientRequestId: 'reply' }) });
    vi.mocked(deps.request).mockRejectedValueOnce(new Error('Offline'));
    expect(await handleHostCommand({ id: 'reply2', type: 'reply', deviceId: 'b', threadId: 't', text: 'hello' }, deps)).toEqual({ ok: false, error: 'Offline' });
  });
  it('resolves an exact pending approval using its real question ID and option label', async () => {
    const deps = dependencies();
    vi.mocked(deps.store.loadDetail).mockResolvedValue({ pendingRequests: [{ id: 'req', kind: 'permissionRequest', questions: [{ id: 'permission', options: [{ label: 'Allow once' }, { label: 'Reject' }] }] }] } as never);
    expect(await handleHostCommand({ id: 'cmd', type: 'resolveRequest', deviceId: 'b', threadId: 't', requestId: 'req', decision: 'approve' }, deps)).toEqual({ ok: true });
    expect(deps.request).toHaveBeenCalledWith('b', '/api/threads/t/requests/req/respond', { method: 'POST', body: JSON.stringify({ answers: { permission: { answers: ['Allow once'] } } }) });
  });
});

it('throttles activity sync to four per second and suppresses unchanged snapshots', async () => {
  vi.useFakeTimers();
  let current = { ...index, threads: [thread('run', { status: 'running' })] };
  let notify: () => void = () => undefined;
  const store = { getSnapshot: () => current, subscribe: (listener: () => void) => { notify = listener; return () => undefined; } } as unknown as CommandDependencies['store'];
  const host = { activity: { sync: vi.fn() }, badge: { set: vi.fn() } } as unknown as import('./host').RemoteCodexHost;
  const { syncActivity } = await import('./activity');
  const stop = syncActivity(store, host, () => null);
  await vi.advanceTimersByTimeAsync(0);
  expect(host.activity.sync).toHaveBeenCalledTimes(1);
  for (let i = 0; i < 20; i++) {
    current = { ...current, threads: [thread('run', { status: 'running', title: `update ${i}` })] };
    notify(); await vi.advanceTimersByTimeAsync(40);
  }
  expect(vi.mocked(host.activity.sync).mock.calls.length).toBeLessThanOrEqual(4);
  await vi.advanceTimersByTimeAsync(250);
  const count = vi.mocked(host.activity.sync).mock.calls.length;
  notify(); await vi.advanceTimersByTimeAsync(30_000);
  expect(host.activity.sync).toHaveBeenCalledTimes(count);
  stop(); vi.useRealTimers();
});
