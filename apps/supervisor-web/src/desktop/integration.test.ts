import { describe, expect, it, vi } from 'vitest';
import { handleHostCommand, resolveRequest, syncActivity, type CommandDependencies } from './integration';
import type { DesktopIndex } from './data/types';
import type { PockymoeHost } from './host';
vi.mock('../lib/api', () => ({ respondToThreadRequest: vi.fn(), sendThreadPrompt: vi.fn() }));
function dependencies() {
  return { store: { markRead: vi.fn(), loadDetail: vi.fn(), acceptDetail: vi.fn(), patchThread: vi.fn() }, navigate: vi.fn(), openMap: vi.fn(), revealInMap: vi.fn(), newThread: vi.fn(), respond: vi.fn(async () => ({})), send: vi.fn(async () => ({})) } as unknown as CommandDependencies;
}
describe('desktop host bridge v2', () => {
  it('dispatches every navigation command and acknowledges read marks', async () => {
    const deps = dependencies();
    for (const command of [{ id: 'nav', type: 'navigate', path: '/devices/a/threads/t' }, { id: 'read', type: 'markRead', key: 'a:t' }, { id: 'map', type: 'openMap' }, { id: 'reveal', type: 'revealInMap', key: 'a:t' }, { id: 'new', type: 'newThread' }, { id: 'devices', type: 'openDevices' }] as const) expect(await handleHostCommand(command, deps)).toEqual({ ok: true });
    expect(deps.store.markRead).toHaveBeenCalledWith('a:t'); expect(deps.openMap).toHaveBeenCalled(); expect(deps.revealInMap).toHaveBeenCalledWith('a:t'); expect(deps.newThread).toHaveBeenCalled(); expect(deps.navigate).toHaveBeenLastCalledWith('/desktop/devices');
    expect((await handleHostCommand({ id: 'bad', type: 'navigate', path: '//outside' }, deps)).ok).toBe(false);
  });
  it('sends a reply using the explicit command device and idempotency ID', async () => {
    const deps = dependencies();
    expect(await handleHostCommand({ id: 'reply', type: 'reply', deviceId: 'b', threadId: 't', text: 'continue' }, deps)).toEqual({ ok: true });
    expect(deps.send).toHaveBeenCalledWith('t', { prompt: 'continue', clientRequestId: 'reply' }, 'b');
    vi.mocked(deps.send!).mockRejectedValueOnce(new Error('设备离线'));
    expect(await handleHostCommand({ id: 'retry', type: 'reply', deviceId: 'b', threadId: 't', text: 'continue' }, deps)).toEqual({ ok: false, error: '设备离线' });
  });
  it('answers the advertised approval question and prefers allow-once', async () => {
    const deps = dependencies();
    vi.mocked(deps.store.loadDetail).mockResolvedValue({ pendingRequests: [{ id: 'request', kind: 'permissionRequest', questions: [{ id: 'permission', options: [{ label: 'Always allow', description: 'allow always' }, { label: 'Allow this time', description: 'allow once' }, { label: 'Reject', description: 'reject once' }] }] }] } as never);
    expect(await handleHostCommand({ id: 'approve', type: 'resolveRequest', deviceId: 'b', threadId: 't', requestId: 'request', decision: 'approve' }, deps)).toEqual({ ok: true });
    expect(deps.respond).toHaveBeenCalledWith('t', 'request', { answers: { permission: { answers: ['Allow this time'] } } }, 'b');
  });
  it('does not fabricate a binary answer for an input question', async () => {
    const deps = dependencies(); vi.mocked(deps.store.loadDetail).mockResolvedValue({ pendingRequests: [{ id: 'input', kind: 'requestUserInput', questions: [] }] } as never);
    await expect(resolveRequest('b', 't', 'input', 'approve', deps)).rejects.toThrow('打开线程'); expect(deps.respond).not.toHaveBeenCalled();
  });
  it('syncs only changes and throttles native activity to <=4/s', async () => {
    vi.useFakeTimers();
    let state = { accountId: 'a', devices: [], workspaces: [], threads: [], activity: {}, extras: {}, hydrated: true, refreshing: false, error: null, pulseKeys: [] } as DesktopIndex;
    let notify: () => void = () => undefined;
    const store = { getSnapshot: () => state, subscribe: (listener: () => void) => { notify = listener; return () => undefined; } } as unknown as CommandDependencies['store'];
    const host = { activity: { sync: vi.fn() }, badge: { set: vi.fn() } } as unknown as PockymoeHost;
    const stop = syncActivity(store, host, () => null, true);
    await vi.advanceTimersByTimeAsync(0); expect(host.activity.sync).toHaveBeenCalledTimes(1);
    for (let i = 0; i < 20; i++) { state = { ...state, accountId: `a${i}` }; notify(); await vi.advanceTimersByTimeAsync(40); }
    expect(vi.mocked(host.activity.sync).mock.calls.length).toBeLessThanOrEqual(4);
    await vi.advanceTimersByTimeAsync(250); const count = vi.mocked(host.activity.sync).mock.calls.length;
    notify(); await vi.advanceTimersByTimeAsync(30000); expect(host.activity.sync).toHaveBeenCalledTimes(count);
    stop(); vi.useRealTimers();
  });
});

it('delivers the empty logout snapshot after route teardown without breaking the throttle', async () => {
  vi.useFakeTimers();
  let state = { accountId: 'a', devices: [], workspaces: [], threads: [{ id: 't', key: 'd:t', deviceId: 'd', title: 'Task', workspaceId: 'w', status: 'running', updatedAt: '2026-11-01T00:00:00Z', requests: [{ id: 'request', kind: 'approval', summary: 'Run checks', createdAt: '2026-11-01T00:00:00Z' }] }] } as unknown as DesktopIndex;
  const store = { getSnapshot: () => state, subscribe: () => () => undefined } as unknown as CommandDependencies['store'];
  const host = { activity: { sync: vi.fn() }, badge: { set: vi.fn() } } as unknown as PockymoeHost;
  const stop = syncActivity(store, host, () => 'd:t', true);
  await vi.advanceTimersByTimeAsync(0); expect(host.badge.set).toHaveBeenLastCalledWith(1, 'attention');
  state = { ...state, threads: [] };
  window.dispatchEvent(new Event('pockymoe-account-logout')); stop();
  await vi.advanceTimersByTimeAsync(249); expect(host.activity.sync).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1);
  expect(host.activity.sync).toHaveBeenLastCalledWith(expect.objectContaining({ items: [], focusedThreadKey: null }));
  expect(host.badge.set).toHaveBeenLastCalledWith(0, 'none');
  vi.useRealTimers();
});
