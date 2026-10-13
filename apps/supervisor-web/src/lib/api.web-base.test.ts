import { afterEach, expect, it, vi } from 'vitest';

vi.mock('./relayTransport', () => ({
  encryptedBrowserFetch: vi.fn(),
  encryptedRelaySocket: vi.fn(),
}));

afterEach(() => {
  document.querySelector('base')?.remove();
  localStorage.clear();
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  vi.resetModules();
});

it.each(['/', '/api/', '/relay/', '/relay/devices/', '/devices/decoy/threads/fake/', '/api/workspaces/W/agents/A/interface/ui/'])(
  'keeps logical API, relay and socket targets distinct from the mount %s',
  async prefix => {
    const base = document.createElement('base');
    base.href = prefix;
    document.head.append(base);
    vi.stubEnv('VITE_POCKYMOE_RELAY_ORIGIN', '');
    vi.resetModules();
    const api = await import('./api');
    const transport = await import('./relayTransport');
    const { scopeFromPage } = await import('./relayTransportCrypto');
    vi.mocked(transport.encryptedBrowserFetch).mockReset().mockImplementation(async () => Response.json({}));
    const fetch = vi.fn(async () => Response.json({}));
    vi.stubGlobal('fetch', fetch);
    localStorage.setItem('remote-codex-relay-mode', 'true');
    api.setSelectedRelayDeviceId('host');
    expect(scopeFromPage(`${window.location.origin}${prefix}devices/peer/threads/thread`, 'peer')).toBe('thread');
    expect(scopeFromPage(`${window.location.origin}${prefix}devices/peer/threads/thread`, 'host')).toBeUndefined();
    expect(scopeFromPage(`${window.location.origin}${prefix}workspaces`, 'decoy')).toBeUndefined();

    // The captured pane wins over the selected device, including normalized
    // legacy/dot-relative inputs. ACL writes must never fall back to plaintext.
    for (const path of ['api/transport/grants', '/api/transport/grants', './api/transport/grants', 'api/../api/transport/grants']) {
      await api.request(path, { method: 'PUT', body: '{}' }, { deviceId: 'peer', threadScope: 'thread' });
    }
    expect(transport.encryptedBrowserFetch).toHaveBeenCalledTimes(4);
    for (const [url, init, scope] of vi.mocked(transport.encryptedBrowserFetch).mock.calls) {
      expect(url).toBe(`${prefix}relay/devices/peer/api/transport/grants`);
      expect(init.method).toBe('PUT');
      expect(scope).toBe('thread');
    }
    expect(fetch).not.toHaveBeenCalled();

    await api.fetchThreadCapabilitySnapshot('thread', 'peer', { load: false });
    await api.releaseThread('thread', {}, 'peer');
    await api.fetchHarnessProcesses();
    await api.fetchHarnessResourceSettings('peer');
    await api.updateHarnessResourceSettings({} as never, 'peer');
    expect(vi.mocked(transport.encryptedBrowserFetch).mock.calls.slice(4).map(([url, init]) => [url, init.method])).toEqual([
      [`${prefix}relay/devices/peer/api/threads/thread/capabilities?load=false`, undefined],
      [`${prefix}relay/devices/peer/api/threads/thread/release`, 'POST'],
      [`${prefix}relay/devices/host/api/device/processes`, undefined],
      [`${prefix}relay/devices/peer/api/harness-resources/settings`, undefined],
      [`${prefix}relay/devices/peer/api/harness-resources/settings`, 'PUT'],
    ]);

    await api.request('relay/portal');
    expect(fetch).toHaveBeenLastCalledWith(`${prefix}relay/portal`, expect.any(Object));
    await api.request('api/version', undefined, { deviceId: null });
    expect(fetch).toHaveBeenLastCalledWith(`${prefix}api/version`, expect.any(Object));
    expect(api.readSelectedRelayDeviceId()).toBe('host');

    const socket = { addEventListener: vi.fn(), send: vi.fn() };
    vi.mocked(transport.encryptedRelaySocket).mockReturnValue(socket as never);
    api.connectShellSocket({}, { deviceId: 'peer', threadId: 'thread' });
    expect(transport.encryptedRelaySocket).toHaveBeenLastCalledWith(
      `${window.location.origin.replace('http:', 'ws:')}${prefix}relay/devices/peer/ws?threadId=thread`,
    );

    vi.stubEnv('VITE_POCKYMOE_RELAY_ORIGIN', 'https://relay.example.test');
    await api.request('api/transport/grants', { method: 'DELETE' }, { deviceId: 'peer' });
    expect(transport.encryptedBrowserFetch).toHaveBeenLastCalledWith(
      'https://relay.example.test/relay/devices/peer/api/transport/grants',
      expect.objectContaining({ method: 'DELETE', credentials: 'include' }),
      undefined,
    );
    await api.request('relay/portal');
    expect(fetch).toHaveBeenLastCalledWith('https://relay.example.test/relay/portal', expect.objectContaining({ credentials: 'include' }));
  },
);
