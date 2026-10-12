import { beforeEach, expect, it, vi } from 'vitest';
const state = vi.hoisted(() => ({ recipient: vi.fn(), exchange: vi.fn(), revocations: vi.fn(), report: null as null | ((status: any) => void) }));
vi.mock('./accountTransport', () => ({ verifySharedDeviceRole: state.recipient }));
vi.mock('./relayTransportCrypto', () => ({
  exchange: state.exchange,
  setTransportReporter: (report: (status: any) => void) => { state.report = report; },
  SocketCipher: class {},
  deviceRoute: vi.fn(),
  trustPinnedDevice: vi.fn(),
  scopeFromPage: (page: string, device: string) => {
    const match = new URL(page).pathname.match(
      /\/devices\/([^/]+)\/threads\/([^/]+)/,
    );
    return match && match[1] === device ? match[2] : undefined;
  },
}));
vi.mock('./accountVaultState', () => ({ accountRevocations: state.revocations }));
import { probeDeviceEncryption, getTransportStatus } from './relayTransport';
beforeEach(() => {
  state.recipient.mockReset().mockResolvedValue(true);
  state.exchange.mockReset().mockResolvedValue({ response: Response.json({}) });
});
it('reconnects a thread recipient through their scoped read route rather than owner metadata', async () => {
  window.history.replaceState(null, '', '/devices/device/threads/thread');
  await probeDeviceEncryption('device');
  expect(state.recipient).toHaveBeenCalledWith('device', 'thread');
  const [request, scope] = state.exchange.mock.calls[0]!;
  expect(request.url).toBe(
    'http://localhost:3000/relay/devices/device/api/threads/thread',
  );
  expect(scope).toBe('thread');
});
it('uses the shared collection for a whole-device recipient', async () => {
  window.history.replaceState(null, '', '/devices/device/workspaces');
  await probeDeviceEncryption('device');
  expect(state.exchange.mock.calls[0]![0].url).toBe(
    'http://localhost:3000/relay/devices/device/api/workspaces',
  );
});
it('keeps the owner probe and never applies another device thread scope', async () => {
  state.recipient.mockResolvedValue(false);
  window.history.replaceState(null, '', '/devices/other/threads/thread');
  await probeDeviceEncryption('device');
  const [request, scope] = state.exchange.mock.calls[0]!;
  expect(request.url).toBe(
    'http://localhost:3000/relay/devices/device/api/version',
  );
  expect(scope).toBeUndefined();
});

it('invalidates a locally revoked device status even when the relay revocation refresh fails', async () => {
  state.report!({ deviceId: 'removed', state: 'encrypted', paired: true, certificateVerified: true, role: 'owner' });
  state.revocations.mockImplementation(async (refresh?: boolean) => { if (refresh) throw new Error('offline'); return [{ kind: 'device', subjectId: 'removed' }]; });
  window.dispatchEvent(new Event('pockymoe-account-revocations-changed'));
  await vi.waitFor(() => expect(getTransportStatus('removed')).toMatchObject({ state: 'identity-changed', certificateVerified: false }));
});
