import { openDevicePortPreview } from './portPreview';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { encryptedBrowserFetch, encryptedRelaySocket } from './relayTransport';
import {
  setSelectedRelayDeviceId,
  fetchThreadDetail,
  fetchThreadModels,
  fetchThreadCapabilitySnapshot,
  sendThreadPrompt,
  updateThreadSettings,
  interruptThread,
  cancelPendingSteer,
  respondToThreadRequest,
  fetchWorkspaceFileTree,
  fetchWorkspaceDocument,
  saveWorkspaceDocument,
  createWorkspaceFile,
  downloadWorkspaceFile,
  buildWorkspaceRawFileUrl,
  buildThreadImageAssetUrl,
  workspaceResourceScope,
  connectShellSocket,
  fetchThreadDelivery,
  dshConsoleUrl,
  loadPluginSettings,
  savePluginSettings,
  threadPluginAdapter,
  ApiError,
} from './api';
vi.mock('./portPreview', () => ({ openDevicePortPreview: vi.fn() }));
vi.mock('./relayTransport', () => ({
  encryptedBrowserFetch: vi.fn(),
  encryptedRelaySocket: vi.fn(),
}));
beforeEach(() => {
  vi.clearAllMocks();
  localStorage.setItem('remote-codex-relay-mode', 'true');
  setSelectedRelayDeviceId('host-device');
  vi.mocked(encryptedBrowserFetch).mockImplementation(
    async () =>
      new Response('{}', { headers: { 'content-type': 'application/json' } }),
  );
});
afterEach(() => {
  localStorage.clear();
  vi.unstubAllGlobals();
});
it('pins plugin settings to their device, round-trips the revision, and preserves 409 conflicts', async () => {
  const snapshot = { revision: 4, values: { model: 'test-model', connectionFile: '/private/profile.json' } };
  vi.mocked(encryptedBrowserFetch).mockResolvedValueOnce(new Response(JSON.stringify(snapshot), { headers: { 'content-type': 'application/json' } }));
  expect(await loadPluginSettings('pockymoe.content-quality', 'settings-device')).toEqual(snapshot);
  setSelectedRelayDeviceId('another-device');
  await savePluginSettings('pockymoe.content-quality', snapshot, 'settings-device');
  const calls = vi.mocked(encryptedBrowserFetch).mock.calls;
  expect(calls[0]![0]).toBe('/relay/devices/settings-device/api/plugins/pockymoe.content-quality/settings');
  expect(calls[1]![0]).toBe(calls[0]![0]);
  expect(calls[0]![1]?.cache).toBe('no-store');
  expect(calls[1]![1]?.method).toBe('PUT');
  expect(JSON.parse(calls[1]![1]?.body as string)).toEqual(snapshot);
  vi.mocked(encryptedBrowserFetch).mockResolvedValueOnce(new Response(JSON.stringify({ code: 'conflict', message: 'Settings changed' }), { status: 409, headers: { 'content-type': 'application/json' } }));
  const error = await savePluginSettings('pockymoe.content-quality', snapshot, 'settings-device').catch(error => error);
  expect(error).toBeInstanceOf(ApiError);
  expect(error.statusCode).toBe(409);
});
it('routes every secondary chat operation and attached prompt to its captured device', async () => {
  const target = 'other-device';
  await fetchThreadDetail('peer', {}, target);
  await fetchThreadModels('peer', target);
  await fetchThreadCapabilitySnapshot('peer', target);
  await sendThreadPrompt('peer', { prompt: 'hello' }, target);
  await sendThreadPrompt(
    'peer',
    {
      prompt: 'file',
      attachments: [
        {
          file: new File(['body'], 'a.txt'),
          clientId: 'a',
          kind: 'file',
          originalName: 'a.txt',
          placeholder: '[FILE a.txt]',
        },
      ],
    },
    target,
  );
  await updateThreadSettings('peer', { model: 'model' }, target);
  await interruptThread('peer', {}, target);
  await cancelPendingSteer('peer', 'queue', target);
  await respondToThreadRequest('peer', 'request', { answers: {} }, target);
  expect(encryptedBrowserFetch).toHaveBeenCalledTimes(9);
  for (const [path] of vi.mocked(encryptedBrowserFetch).mock.calls)
    expect(path).toMatch(/^\/relay\/devices\/other-device\/api\/threads\/peer/);
  expect(
    vi.mocked(encryptedBrowserFetch).mock.calls[4]![1]?.body,
  ).toBeInstanceOf(FormData);
  expect(localStorage.getItem('remote-codex-relay-device-id')).toBe(
    'host-device',
  );
});
it('pins workspace reads, creation, saves, downloads, media and draft scope to the tool target', async () => {
  await fetchWorkspaceFileTree('ws', {}, 'peer-device');
  await fetchWorkspaceDocument('ws', 'a.md', undefined, 'peer-device');
  await createWorkspaceFile('ws', 'a.md', 'peer-device');
  await saveWorkspaceDocument(
    'ws',
    {
      path: 'a.md',
      operationId: 'op',
      draftRevision: 1,
      content: 'b',
      workspaceRevision: 'rev',
      fileIdentity: 'file',
      contentHash: 'hash',
      encoding: 'utf8',
      bom: false,
      eol: 'lf',
    } as never,
    'peer-device',
  );
  await downloadWorkspaceFile('ws', { path: 'a.md' }, 'peer-device');
  for (const [path] of vi.mocked(encryptedBrowserFetch).mock.calls)
    expect(path).toMatch(
      /^\/relay\/devices\/peer-device\/api\/workspaces\/ws\//,
    );
  expect(
    buildWorkspaceRawFileUrl('ws', { path: 'a.md' }, 'peer-device'),
  ).toContain('/relay/devices/peer-device/api/workspaces/ws/');
  expect(
    buildThreadImageAssetUrl('peer', { path: 'photo.png' }, 'peer-device'),
  ).toContain('/relay/devices/peer-device/api/threads/peer/');
  expect(workspaceResourceScope('peer-device')).not.toBe(
    workspaceResourceScope('host-device'),
  );
});
it('retains the explicit device through an older-supervisor delivery fallback', async () => {
  vi.mocked(encryptedBrowserFetch).mockResolvedValueOnce(
    new Response(
      JSON.stringify({ code: 'badRequest', message: 'unsupported' }),
      { status: 400, headers: { 'content-type': 'application/json' } },
    ),
  );
  await fetchThreadDelivery('peer', 'peer-device');
  expect(encryptedBrowserFetch).toHaveBeenCalledTimes(2);
  for (const [path] of vi.mocked(encryptedBrowserFetch).mock.calls)
    expect(path).toMatch(
      /^\/relay\/devices\/peer-device\/api\/threads\/peer\?/,
    );
});
it('subscribes the terminal to the target device and thread instead of the primary route', () => {
  const socket = { addEventListener: vi.fn(), send: vi.fn() };
  vi.mocked(encryptedRelaySocket).mockReturnValue(socket as never);
  connectShellSocket({}, { deviceId: 'peer-device', threadId: 'peer-thread' });
  const url = new URL(vi.mocked(encryptedRelaySocket).mock.calls[0]![0]);
  expect(url.pathname).toBe('/relay/devices/peer-device/ws');
  expect(url.searchParams.get('threadId')).toBe('peer-thread');
});

it('uses the relay origin for device HTTP, media and sockets while keeping storage on the Web origin', async () => {
  vi.stubEnv('VITE_POCKYMOE_RELAY_ORIGIN', 'https://relay.example.test');
  try {
    await fetchThreadDetail('peer', {}, 'peer-device');
    expect(vi.mocked(encryptedBrowserFetch).mock.calls[0]![0]).toMatch(/^https:\/\/relay\.example\.test\/relay\/devices\/peer-device\/api\//);
    expect(vi.mocked(encryptedBrowserFetch).mock.calls[0]![1]?.credentials).toBe('include');
    expect(buildThreadImageAssetUrl('peer', { path: 'photo.png' }, 'peer-device')).toMatch(/^https:\/\/relay\.example\.test\/relay\/devices\/peer-device\/api\//);
    const socket = { addEventListener: vi.fn(), send: vi.fn() };
    vi.mocked(encryptedRelaySocket).mockReturnValue(socket as never);
    connectShellSocket({}, { deviceId: 'peer-device', threadId: 'peer' });
    expect(vi.mocked(encryptedRelaySocket).mock.calls[0]![0]).toBe('wss://relay.example.test/relay/devices/peer-device/ws?threadId=peer');
    expect(workspaceResourceScope('peer-device')).toContain(window.location.origin);
    expect(workspaceResourceScope('peer-device')).not.toContain('relay.example.test');
  } finally {
    vi.unstubAllEnvs();
  }
});

it.each([
  ['grant_denied', /Your shared access does not permit this action.*read-only/],
  ['owner_required', /Only the device owner/],
])('reports %s as permission information without asking to unlock or retrying', async (code, message) => {
  const unlock = vi.fn();
  window.addEventListener('remote-codex-owner-unlock-request', unlock);
  window.addEventListener('remote-codex-owner-read-blocked', unlock);
  try {
    vi.mocked(encryptedBrowserFetch).mockResolvedValue(Response.json({ code, message: 'denied' }, { status: 403 }));
    await expect(sendThreadPrompt('peer', { prompt: 'hello' }, 'peer-device')).rejects.toMatchObject({
      statusCode: 403, payload: { code }, message: expect.stringMatching(message),
    });
    expect(encryptedBrowserFetch).toHaveBeenCalledTimes(1);
    expect(unlock).not.toHaveBeenCalled();
  } finally {
    window.removeEventListener('remote-codex-owner-unlock-request', unlock);
    window.removeEventListener('remote-codex-owner-read-blocked', unlock);
  }
});

it('reuses the signed DSH console lease when opening its existing mapping', async () => {
  vi.mocked(openDevicePortPreview).mockResolvedValue({ url: 'https://preview.invalid/console' });
  const target = { port: 4242, path: '/?token=launch', mappingId: 'console-mapping', leaseId: 'signed-console-lease', expiresAt: Date.now() + 10000 };
  expect(await dshConsoleUrl('thread', target, 'peer-device')).toBe('https://preview.invalid/console');
  expect(openDevicePortPreview).toHaveBeenCalledWith('peer-device', 'console-mapping', target.path, undefined, { leaseId: target.leaseId, expiresAt: target.expiresAt });
  expect(encryptedBrowserFetch).not.toHaveBeenCalled();
});

it('routes one conversation\'s plugin scope to its own device and keeps 409 conflicts', async () => {
  const adapter = threadPluginAdapter('thread-device');
  setSelectedRelayDeviceId('another-device');
  await adapter.fetchPlugins('t 1');
  await adapter.loadSettings('t 1', 'pockymoe.content-quality');
  const input = { revision: 3, enabled: false, values: null };
  await adapter.saveSettings('t 1', 'pockymoe.content-quality', input);
  const calls = vi.mocked(encryptedBrowserFetch).mock.calls;
  expect(calls.map(call => call[0])).toEqual([
    '/relay/devices/thread-device/api/threads/t%201/plugins',
    '/relay/devices/thread-device/api/threads/t%201/plugins/pockymoe.content-quality/settings',
    '/relay/devices/thread-device/api/threads/t%201/plugins/pockymoe.content-quality/settings',
  ]);
  expect(calls[0]![1]?.cache).toBe('no-store');
  expect(calls[2]![1]?.method).toBe('PUT');
  expect(JSON.parse(calls[2]![1]?.body as string)).toEqual(input);
  vi.mocked(encryptedBrowserFetch).mockResolvedValueOnce(new Response(JSON.stringify({ code: 'conflict', message: 'changed' }), { status: 409, headers: { 'content-type': 'application/json' } }));
  const error = await adapter.saveSettings('t 1', 'pockymoe.content-quality', input).catch(caught => caught);
  expect(error).toBeInstanceOf(ApiError);
  expect((error as ApiError).statusCode).toBe(409);
});
