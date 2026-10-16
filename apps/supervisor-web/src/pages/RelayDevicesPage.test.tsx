import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import type { RelayDeviceDto } from '@pockymoe/shared';
import { ApiError, createRelayDevice, fetchAgentIsolation, fetchRelayPortal, request, setDeviceIsolation } from '../lib/api';
import { requireUnlock } from '../lib/accountVault';
import { accountPasswordProof } from '../lib/accountAuth';
import { prepareDeviceEnrollment } from '../lib/deviceEnrollment';
import { securityRequest } from '../lib/relaySecurity';
import { relaySupervisorCommand } from '../lib/deviceSetupCommands';
import { useDeviceProtection } from '../lib/useDeviceProtection';
import { RelayDevicesPage } from './RelayDevicesPage';
import { protectDevices, deviceProtectionProgress } from '../lib/remoteDeviceProtection';

vi.mock('../lib/api', () => ({
  ApiError: class extends Error {
    constructor(public statusCode: number, public payload: { code: string; message: string }) { super(payload.message); }
  },
  request: vi.fn(), createRelayDevice: vi.fn(), fetchRelayPortal: vi.fn(),
  createRelayGrant: vi.fn(), enableRelayMode: vi.fn(), revokeRelayGrant: vi.fn(),
  revokeRelayShare: vi.fn(), setSelectedRelayDeviceId: vi.fn(),
  setSelectedRelayThreadId: vi.fn(), updateRelayGrant: vi.fn(), updateRelayShare: vi.fn(),
  fetchAgentIsolation: vi.fn(), setDeviceIsolation: vi.fn(),
}));
vi.mock('../lib/accountVault', () => ({ requireUnlock: vi.fn() }));
vi.mock('../lib/accountAuth', () => ({ accountPasswordProof: vi.fn() }));
vi.mock('../lib/deviceEnrollment', () => ({ prepareDeviceEnrollment: vi.fn() }));
vi.mock('../lib/deviceRemoval', () => ({ removeDeviceFromAccount: vi.fn() }));
vi.mock('../lib/signedSharing', () => ({ sharingEditInput: vi.fn() }));
vi.mock('../lib/relaySecurity', async (original) => ({ ...await original<object>(), securityRequest: vi.fn() }));
vi.mock('../lib/relayTransport', () => ({ getTransportStatus: vi.fn() }));
vi.mock('../lib/relayOrigin', () => ({ relayOrigin: () => 'https://relay.test' }));
vi.mock('../lib/useDevicePresence', () => ({ useDevicePresence: (portal: unknown) => portal }));
vi.mock('../lib/useDeviceProtection', () => ({ useDeviceProtection: vi.fn(() => 'not-protected'), protectionLabel: () => 'devices.notProtected' }));
vi.mock('../components/DeviceBindingPanel', () => ({ DeviceBindingPanel: () => null }));
vi.mock('../lib/remoteDeviceProtection', () => ({
  canSwitchLocalEnrollmentToRemote: vi.fn(async () => false), switchLocalEnrollmentToRemote: vi.fn(),
  protectDevices: vi.fn(), deviceProtectionProgress: vi.fn(), updateAndProtectDevice: vi.fn(),
  REMOTE_PROTECTION_CHANGED_EVENT: 'pockymoe-remote-protection-changed',
}));
vi.mock('../components/DeviceEncryptionStatus', () => ({ DeviceEncryptionStatus: () => null }));
vi.mock('../components/SignedShareDetails', () => ({ SignedShareDetails: () => null, OrphanDeviceGrants: () => null }));
vi.mock('../components/SharedAccessCard', () => ({ SharedAccessCard: () => null }));
vi.mock('../components/ProductHeader', () => ({ ProductHeader: () => null }));
vi.mock('../components/onboarding/TourProvider', () => ({ TourLauncherButton: () => null }));

const device: RelayDeviceDto = {
  id: 'test-device', ownerUserId: 'test-user', name: 'Test laptop', token: null,
  tokenPreview: 'preview', connected: false, connectedAt: null,
  lastHeartbeatAt: null, createdAt: '2026-01-01T00:00:00Z',
};
const binding = {
  deviceId: device.id, userId: 'test-user', accountKeyId: 'test-account-key-id',
  createdAt: 1, accountPublicKey: 'test-account-key', enrollSecret: 'test-enrollment-secret',
};
const token = 'test-token-0123456789';
const writeText = vi.fn();
const reauthenticationRequired = () => new ApiError(403, { code: 'reauthentication_required', message: 'Verify again' });

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(useDeviceProtection).mockReturnValue('not-protected');
  vi.mocked(deviceProtectionProgress).mockResolvedValue(undefined);
  vi.mocked(protectDevices).mockResolvedValue(undefined);
  vi.mocked(fetchRelayPortal).mockResolvedValue({ user: { id: 'test-user' } as never, devices: [device], sharedWithMe: [], sharedByMe: [] });
  vi.mocked(createRelayDevice).mockResolvedValue({ device, token });
  vi.mocked(request).mockResolvedValue({ token });
  vi.mocked(prepareDeviceEnrollment).mockResolvedValue(binding);
  vi.mocked(accountPasswordProof).mockResolvedValue({ authKey: 'test-proof' });
  vi.mocked(securityRequest).mockImplementation(async (path) => (path === ''
    ? { authenticatorEnabled: false, passkeys: [], recentlyVerified: false }
    : {}) as never);
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  writeText.mockResolvedValue(undefined);
});
afterEach(() => vi.unstubAllGlobals());

async function openPage() {
  const view = render(<MemoryRouter><RelayDevicesPage /></MemoryRouter>);
  await screen.findByText(device.name);
  return view;
}
async function addDevice() {
  fireEvent.click(screen.getByRole('button', { name: 'Add device' }));
  fireEvent.change(screen.getByLabelText('Device name'), { target: { value: 'New laptop' } });
  fireEvent.click(screen.getByRole('button', { name: 'Create token' }));
}
function copySetup(platform: 'unix' | 'windows' = 'unix') {
  fireEvent.click(screen.getByRole('button', { name: `More actions for ${device.name}` }));
  fireEvent.click(screen.getByRole('menuitem', { name: platform === 'unix' ? 'Copy setup for macOS/Linux' : 'Copy setup for Windows' }));
}
async function verify() {
  const dialog = await screen.findByRole('dialog', { name: 'Verify your identity' });
  fireEvent.change(within(dialog).getByLabelText('Password'), { target: { value: 'test-password' } });
  fireEvent.click(within(dialog).getByRole('button', { name: 'Verify' }));
}

describe('device setup reauthentication', () => {
  it('offers Protect all devices for the owned device list and confirms before starting the batch', async () => {
    await openPage();
    fireEvent.click(screen.getByRole('button', { name: 'Protect all devices' }));
    expect(protectDevices).not.toHaveBeenCalled();
    const dialog = screen.getByRole('dialog', { name: 'Protect all devices' });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Protect devices' }));
    await waitFor(() => expect(protectDevices).toHaveBeenCalledExactlyOnceWith([device]));
  });
  it('reverifies Add device and retries the original name once before creating enrollment', async () => {
    vi.mocked(createRelayDevice).mockRejectedValueOnce(reauthenticationRequired());
    // The protected endpoint's rejection wins over a racing "recent" status.
    vi.mocked(securityRequest).mockResolvedValueOnce({ authenticatorEnabled: false, passkeys: [], recentlyVerified: true });
    await openPage();
    await addDevice();
    await screen.findByRole('dialog', { name: 'Verify your identity' });
    expect(createRelayDevice).toHaveBeenCalledExactlyOnceWith({ name: 'New laptop' });
    expect(prepareDeviceEnrollment).not.toHaveBeenCalled();
    await verify();
    await waitFor(() => expect(prepareDeviceEnrollment).toHaveBeenCalledExactlyOnceWith(device.id));
    expect(createRelayDevice).toHaveBeenCalledTimes(2);
    expect(vi.mocked(createRelayDevice).mock.calls[1]).toEqual([{ name: 'New laptop' }]);
    expect(requireUnlock).toHaveBeenCalledExactlyOnceWith('bind-device');
    expect(accountPasswordProof).toHaveBeenCalledWith('test-password', false);
    expect(securityRequest).toHaveBeenCalledWith('/reauth', 'POST', { authKey: 'test-proof' }, 'default');
    expect(screen.queryByRole('dialog')).toBeNull();
  });
  it.each(['unix', 'windows'] as const)('reverifies %s setup copying without replacing the pending secret', async (platform) => {
    vi.mocked(request).mockRejectedValueOnce(reauthenticationRequired());
    await openPage();
    copySetup(platform);
    await screen.findByRole('dialog', { name: 'Verify your identity' });
    expect(writeText).not.toHaveBeenCalled();
    await verify();
    await waitFor(() => expect(writeText).toHaveBeenCalledExactlyOnceWith(relaySupervisorCommand(token, platform, binding)));
    expect(prepareDeviceEnrollment).toHaveBeenCalledExactlyOnceWith(device.id);
    expect(request).toHaveBeenCalledTimes(2);
    for (const call of vi.mocked(request).mock.calls) expect(call).toEqual([`relay/devices/${device.id}/setup-token`, { method: 'POST' }]);
  });
  it('starts a promised clipboard write within the original click and waits for reauthentication', async () => {
    let contents: Record<string, Promise<Blob>> = {};
    vi.stubGlobal('ClipboardItem', class { constructor(value: Record<string, Promise<Blob>>) { contents = value; } });
    const write = vi.fn(async () => { await contents['text/plain']; });
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText, write } });
    vi.mocked(request).mockRejectedValueOnce(reauthenticationRequired());
    await openPage();
    copySetup();
    expect(write).toHaveBeenCalledTimes(1);
    expect(request).not.toHaveBeenCalled();
    await verify();
    await screen.findByText('Setup command copied.');
    const blob = await contents['text/plain'];
    expect(await new Promise((resolve) => {
      const reader = new FileReader(); reader.onload = () => resolve(reader.result); reader.readAsText(blob!);
    })).toBe(relaySupervisorCommand(token, 'unix', binding));
    expect(write).toHaveBeenCalledTimes(1);
    expect(writeText).not.toHaveBeenCalled();
  });
  it.each(['add', 'copy'] as const)('cancelling %s verification does not retry the mutation', async (action) => {
    const mutation = action === 'add' ? createRelayDevice : request;
    vi.mocked(mutation).mockRejectedValueOnce(reauthenticationRequired());
    await openPage();
    if (action === 'add') await addDevice(); else copySetup();
    const dialog = await screen.findByRole('dialog', { name: 'Verify your identity' });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(mutation).toHaveBeenCalledTimes(1);
    expect(writeText).not.toHaveBeenCalled();
    expect(screen.queryByRole('alert')).toBeNull();
  });
  it('stops retrying if the relay still rejects Add device after successful verification', async () => {
    vi.mocked(createRelayDevice).mockRejectedValue(reauthenticationRequired());
    await openPage(); await addDevice(); await verify();
    expect(await screen.findByRole('alert')).toHaveTextContent('Verify again');
    expect(createRelayDevice).toHaveBeenCalledTimes(2);
    expect(securityRequest).toHaveBeenCalledTimes(2);
    expect(prepareDeviceEnrollment).not.toHaveBeenCalled();
  });
  it.each([[403, 'forbidden'], [401, 'reauthentication_required'], [500, 'internal_error']] as const)('does not reverify unrelated %s %s failures', async (status, code) => {
    vi.mocked(createRelayDevice).mockRejectedValue(new ApiError(status, { code, message: 'Other failure' }));
    await openPage(); await addDevice();
    expect(await screen.findByRole('alert')).toHaveTextContent('Other failure');
    expect(securityRequest).not.toHaveBeenCalled();
    expect(createRelayDevice).toHaveBeenCalledTimes(1);
  });
  it('does not retry a pending setup request when the page unmounts', async () => {
    vi.mocked(request).mockRejectedValueOnce(reauthenticationRequired());
    const view = await openPage(); copySetup();
    await screen.findByRole('dialog', { name: 'Verify your identity' });
    view.unmount();
    await act(async () => {});
    expect(request).toHaveBeenCalledTimes(1);
    expect(writeText).not.toHaveBeenCalled();
  });
  it('does not start verification when a rejected setup request arrives after unmount', async () => {
    let reject!: (error: unknown) => void;
    vi.mocked(request).mockReturnValueOnce(new Promise((_resolve, decline) => { reject = decline; }));
    const view = await openPage(); copySetup();
    await waitFor(() => expect(request).toHaveBeenCalledTimes(1));
    view.unmount();
    await act(async () => { reject(reauthenticationRequired()); });
    expect(securityRequest).not.toHaveBeenCalled();
    expect(writeText).not.toHaveBeenCalled();
  });
});

it('keeps the local command accessible for a pending enrollment on the Devices page', async () => {
  vi.mocked(useDeviceProtection).mockReturnValue('pending');
  await openPage();
  // The row keeps a short label; the command is one tap behind it.
  fireEvent.click(await screen.findByRole('button', {name:'Waiting for bind'}));
  fireEvent.click(screen.getByRole('button', {name:'Show local bind command'}));
  expect(screen.getByRole('dialog', {name:'Protect this device'})).toBeVisible();
});

describe('agent isolation', () => {
  it('reads and changes isolation on the device, then marks the row', async () => {
    const online = { ...device, connected: true };
    vi.mocked(fetchRelayPortal).mockResolvedValue({ user: { id: 'test-user' } as never, devices: [online], sharedWithMe: [], sharedByMe: [] });
    vi.mocked(fetchAgentIsolation).mockResolvedValue({ device: false, workspaces: [] });
    vi.mocked(setDeviceIsolation).mockResolvedValue({ device: true, workspaces: [] });
    await openPage();
    expect(fetchAgentIsolation).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: `More actions for ${device.name}` }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Agent isolation…' }));
    const dialog = await screen.findByRole('dialog', { name: `Agent isolation · ${device.name}` });
    const toggle = await within(dialog).findByRole('checkbox', { name: 'Isolate this device' });
    expect(fetchAgentIsolation).toHaveBeenCalledWith(device.id);
    expect(toggle).not.toBeChecked();
    fireEvent.click(toggle);
    await waitFor(() => expect(setDeviceIsolation).toHaveBeenCalledExactlyOnceWith(true, device.id));
    await waitFor(() => expect(toggle).toBeChecked());
    expect(screen.getByText('Isolated')).toBeTruthy();
  });

  it('asks to update a device whose runtime predates isolation', async () => {
    const online = { ...device, connected: true };
    vi.mocked(fetchRelayPortal).mockResolvedValue({ user: { id: 'test-user' } as never, devices: [online], sharedWithMe: [], sharedByMe: [] });
    vi.mocked(fetchAgentIsolation).mockRejectedValue(new ApiError(404, { code: 'not_found', message: 'Not found' }));
    await openPage();
    fireEvent.click(screen.getByRole('button', { name: `More actions for ${device.name}` }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Agent isolation…' }));
    expect(await screen.findByText('Update this device to change this.')).toBeTruthy();
    expect(screen.queryByRole('checkbox')).toBeNull();
  });
});
