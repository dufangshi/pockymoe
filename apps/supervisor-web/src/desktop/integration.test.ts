import { describe, expect, it, vi } from 'vitest';
import { translate } from '@pockymoe/thread-ui/i18n';
import { handleHostCommand, resolveRequest, syncActivity, type CommandDependencies } from './integration';
import type { DesktopIndex } from './data/types';
import type { PockymoeHost } from './host';
vi.mock('../lib/api', () => ({ deviceRequest: vi.fn(), respondToThreadRequest: vi.fn(), sendThreadPrompt: vi.fn() }));
function dependencies() {
  return { store: { markRead: vi.fn(), loadRequestDetail: vi.fn(), acceptDetail: vi.fn(), patchThread: vi.fn() }, navigate: vi.fn(), openMap: vi.fn(), revealInMap: vi.fn(), newThread: vi.fn(), respond: vi.fn(async () => ({})), send: vi.fn(async () => ({})) } as unknown as CommandDependencies;
}
describe('spoken summary commands', () => {
  const messages = [
    { role: 'user', text: 'Old ask', turnId: 'T1' }, { role: 'assistant', text: 'Old answer', turnId: 'T1' },
    { role: 'user', text: 'Fix login', turnId: 'T2' }, { role: 'assistant', text: 'Looking', turnId: 'T2' }, { role: 'assistant', text: 'Login fixed.', turnId: 'T2' },
    { role: 'user', text: 'Next', turnId: 'T3' },
  ];
  const data = (result: { data?: unknown }) => result.data as Record<string, unknown>;
  it('reads the named turn, its request and its final answer', async () => {
    const deps = { ...dependencies(), request: vi.fn().mockResolvedValue({ messages }) };
    const result = await handleHostCommand({ id: 'x', type: 'turnText', deviceId: 'b', threadId: 't/1', turnId: 'T2' }, deps as never);
    expect(deps.request).toHaveBeenCalledWith('b', 'api/threads/t%2F1/summary?turns=2&maxChars=20000');
    expect(data(result)).toEqual({ request: 'Fix login', answer: 'Login fixed.' });
  });
  it('falls back to the newest turn when the id is unknown', async () => {
    const deps = { ...dependencies(), request: vi.fn().mockResolvedValue({ messages: messages.slice(0, 5) }) };
    expect(data(await handleHostCommand({ id: 'x', type: 'turnText', deviceId: 'b', threadId: 't', turnId: 'gone' }, deps as never))).toEqual({ request: 'Fix login', answer: 'Login fixed.' });
  });
  it('lists local connections of connected devices and skips one that fails', async () => {
    const store = { ...dependencies().store, getSnapshot: () => ({ devices: [{ id: 'a', name: 'Mac', connected: true }, { id: 'b', name: 'Old', connected: true }, { id: 'c', name: 'Off', connected: false }] }) };
    const request = vi.fn(async (device: string, path: string) => {
      if (device === 'b') throw new Error('404');
      if (path.endsWith('/supervisor')) return { runningVersion: '0.13.41' };
      return { connections: [{ id: 'k', name: 'Main', protocol: 'openai_chat', model: 'm', local: true }, { id: 'r', name: 'Elsewhere', protocol: 'openai_chat', local: false }] };
    });
    const result = await handleHostCommand({ id: 'x', type: 'listConnections' }, { ...dependencies(), store, request } as never);
    expect(result.data).toEqual([{ deviceId: 'a', deviceName: 'Mac', id: 'k', name: 'Main', protocol: 'openai_chat', model: 'm', supervisorVersion: '0.13.41', needsUpdate: true }]);
    expect(request).not.toHaveBeenCalledWith('c', expect.anything());
  });
  it('completes through the saved connection on its own device', async () => {
    const request = vi.fn().mockResolvedValue({ text: 'Done.', model: 'm' });
    const result = await handleHostCommand({ id: 'x', type: 'completeWithConnection', deviceId: 'a', connectionId: 'k', model: 'm', system: 's', user: 'u' }, { ...dependencies(), request } as never);
    expect(request).toHaveBeenCalledWith('a', 'api/management/connections/k/complete', { method: 'POST', body: JSON.stringify({ model: 'm', system: 's', user: 'u' }) });
    expect(result).toEqual({ ok: true, data: { text: 'Done.' } });
  });
  it('reads a connection\'s models with the Settings probe and opens the upstreams page', async () => {
    const request = vi.fn().mockResolvedValue({ models: [{ id: 'a', name: 'Model A' }, { id: 'b' }] });
    const result = await handleHostCommand({ id: 'x', type: 'connectionModels', deviceId: 'd', connectionId: 'k' }, { ...dependencies(), request } as never);
    expect(request).toHaveBeenCalledWith('d', 'api/management/connections/k/models', { method: 'POST', body: '{}' });
    expect(result.data).toEqual([{ id: 'a', name: 'Model A' }, { id: 'b', name: 'b' }]);
    const openUpstreams = vi.fn();
    expect(await handleHostCommand({ id: 'y', type: 'openUpstreams' }, { ...dependencies(), openUpstreams } as never)).toEqual({ ok: true });
    expect(openUpstreams).toHaveBeenCalled();
  });
  it('lists what can be silenced, conversations only and newest first', async () => {
    const store = { ...dependencies().store, getSnapshot: () => ({
      devices: [{ id: 'd', name: 'Mac' }], workspaces: [{ key: 'd:w', deviceId: 'd', label: 'Repo' }],
      threads: [
        { key: 'd:old', id: 'old', deviceId: 'd', workspaceId: 'w', title: 'Old', lastTurnStartedAt: '2026-10-01T00:00:00Z' },
        { key: 'd:new', id: 'new', deviceId: 'd', workspaceId: 'w', title: 'New', lastTurnStartedAt: '2026-11-01T00:00:00Z' },
        { key: 'd:agent', id: 'agent', rootThreadId: 'new', deviceId: 'd', workspaceId: 'w', title: 'Agent', lastTurnStartedAt: '2026-11-02T00:00:00Z' },
      ] }) };
    const result = await handleHostCommand({ id: 'x', type: 'listMuteTargets' }, { ...dependencies(), store } as never);
    expect(result.data).toEqual({ devices: [{ id: 'd', label: 'Mac' }], workspaces: [{ id: 'd:w', label: 'Repo · Mac' }], threads: [{ id: 'd:new', label: 'New · Repo' }, { id: 'd:old', label: 'Old · Repo' }] });
  });
  it('explains a device that cannot complete yet', async () => {
    const request = vi.fn().mockRejectedValue(Object.assign(new Error('Not found'), { statusCode: 404 }));
    expect(await handleHostCommand({ id: 'x', type: 'completeWithConnection', deviceId: 'a', connectionId: 'k', model: '', system: 's', user: 'u' }, { ...dependencies(), request } as never)).toEqual({ ok: false, code: 'supervisorTooOld', error: translate('settings.connUpdateSupervisor') });
  });
});
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
    vi.mocked(deps.store.loadRequestDetail).mockResolvedValue({ pendingRequests: [{ id: 'request', kind: 'permissionRequest', questions: [{ id: 'permission', options: [{ label: 'Always allow', description: 'allow always' }, { label: 'Allow this time', description: 'allow once' }, { label: 'Reject', description: 'reject once' }] }] }] } as never);
    expect(await handleHostCommand({ id: 'approve', type: 'resolveRequest', deviceId: 'b', threadId: 't', requestId: 'request', decision: 'approve' }, deps)).toEqual({ ok: true });
    expect(deps.respond).toHaveBeenCalledWith('t', 'request', { answers: { permission: { answers: ['Allow this time'] } } }, 'b');
  });
  it('does not fabricate a binary answer for an input question', async () => {
    const deps = dependencies(); vi.mocked(deps.store.loadRequestDetail).mockResolvedValue({ pendingRequests: [{ id: 'input', kind: 'requestUserInput', questions: [] }] } as never);
    await expect(resolveRequest('b', 't', 'input', 'approve', deps)).rejects.toMatchObject({ code: 'needsThread' }); expect(deps.respond).not.toHaveBeenCalled();
  });
  it.each([['approve', true], ['deny', false]] as const)('submits %s for generic permission requests without questions', async (decision, allow) => {
    const deps = dependencies();
    vi.mocked(deps.store.loadRequestDetail).mockResolvedValue({ pendingRequests: [{ id: 'request', kind: 'permissionRequest', questions: [] }] } as never);
    expect(await handleHostCommand({ id: 'decision', type: 'resolveRequest', deviceId: 'b', threadId: 't', requestId: 'request', decision }, deps)).toEqual({ ok: true });
    expect(deps.store.loadRequestDetail).toHaveBeenCalledWith('b', 't');
    expect(deps.respond).toHaveBeenCalledWith('t', 'request', { allow }, 'b');
    expect(deps.store.acceptDetail).toHaveBeenCalledWith('b', {});
  });
  it('returns a machine-readable needsThread reason for nonbinary questions', async () => {
    const deps = dependencies();
    vi.mocked(deps.store.loadRequestDetail).mockResolvedValue({ pendingRequests: [{ id: 'input', kind: 'requestUserInput', questions: [{ id: 'environment', options: [{ label: 'Preview' }, { label: 'Staging' }] }] }] } as never);
    expect(await handleHostCommand({ id: 'decision', type: 'resolveRequest', deviceId: 'b', threadId: 't', requestId: 'input', decision: 'approve' }, deps)).toMatchObject({ ok: false, code: 'needsThread' });
    expect(deps.respond).not.toHaveBeenCalled();
  });
  it('rejects stale request IDs instead of answering a different current request', async () => {
    const deps = dependencies(); vi.mocked(deps.store.loadRequestDetail).mockResolvedValue({ pendingRequests: [] } as never);
    expect(await handleHostCommand({ id: 'stale', type: 'resolveRequest', deviceId: 'b', threadId: 't', requestId: 'old', decision: 'deny' }, deps)).toMatchObject({ ok: false });
    expect(deps.respond).not.toHaveBeenCalled();
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

it('allows a genuinely binary input request but preserves its advertised answer value', async () => {
  const deps = dependencies();
  vi.mocked(deps.store.loadRequestDetail).mockResolvedValue({ pendingRequests: [{ id: 'binary', kind: 'requestUserInput', questions: [{ id: 'confirm', options: [{ label: 'Yes', description: 'Proceed' }, { label: 'No', description: 'Stop' }] }] }] } as never);
  expect(await handleHostCommand({ id: 'binary', type: 'resolveRequest', deviceId: 'b', threadId: 't', requestId: 'binary', decision: 'deny' }, deps)).toEqual({ ok: true });
  expect(deps.respond).toHaveBeenCalledWith('t', 'binary', { answers: { confirm: { answers: ['No'] } } }, 'b');
});

it.each([['approve', 'Implement'], ['deny', 'Keep planning']] as const)('handles the binary plan %s action', async (decision, answer) => {
  const deps = dependencies();
  vi.mocked(deps.store.loadRequestDetail).mockResolvedValue({ pendingRequests: [{ id: 'plan', kind: 'planDecision', questions: [{ id: 'decision', options: [{ label: 'Implement', description: 'Implement this plan' }, { label: 'Keep planning', description: 'Keep planning' }] }] }] } as never);
  expect(await handleHostCommand({ id: 'plan', type: 'resolveRequest', deviceId: 'b', threadId: 't', requestId: 'plan', decision }, deps)).toEqual({ ok: true });
  expect(deps.respond).toHaveBeenCalledWith('t', 'plan', { answers: { decision: { answers: [answer] } } }, 'b');
});
