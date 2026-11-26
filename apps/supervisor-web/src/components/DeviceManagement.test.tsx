import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { RelayDeviceDto } from '@pockymoe/shared';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { ApiError, createRelayDevice, fetchAgentIsolation, fetchRelayPortal, request, setDeviceIsolation } from '../lib/api';
import { requireUnlock } from '../lib/accountVault';
import { accountPasswordProof } from '../lib/accountAuth';
import { prepareDeviceEnrollment } from '../lib/deviceEnrollment';
import { securityRequest } from '../lib/relaySecurity';
import { relaySupervisorCommand } from '../lib/deviceSetupCommands';
import { useDeviceProtection } from '../lib/useDeviceProtection';
import { DeviceManagement, DeviceStatusLine } from './DeviceManagement';
import { UnprotectedDevicesButton } from '../desktop/devices/DevicesNavigation';
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
vi.mock('./DeviceBindingPanel', () => ({ DeviceBindingPanel: () => null }));
// The Supervisor block of a device's settings has its own test (RuntimeManagement.test.tsx).
vi.mock('./RuntimeManagement', () => ({ RuntimeManagement: ({ deviceId }: { deviceId: string }) => <p>Supervisor of {deviceId}</p> }));
// The line under a new device's command has its own test (DeviceSetupProgress.test.tsx).
vi.mock('./DeviceSetupProgress', () => ({ DeviceSetupProgress: () => null }));
vi.mock('../lib/remoteDeviceProtection', () => ({
  canSwitchLocalEnrollmentToRemote: vi.fn(async () => false), switchLocalEnrollmentToRemote: vi.fn(),
  protectDevices: vi.fn(), deviceProtectionProgress: vi.fn(), updateAndProtectDevice: vi.fn(),
  REMOTE_PROTECTION_CHANGED_EVENT: 'pockymoe-remote-protection-changed',
}));
vi.mock('./DeviceEncryptionStatus', () => ({ DeviceEncryptionStatus: () => null }));
vi.mock('./SignedShareDetails', () => ({ SignedShareDetails: () => null, OrphanDeviceGrants: () => null }));

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
  setLocale('en', false);
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

// The device page's Settings tab (issue #115): name, connection and isolation of one device.
async function openSettingsPane() {
  const view = render(<MemoryRouter initialEntries={[`/desktop/devices/${device.id}?tab=settings`]}><DeviceManagement pane="settings" deviceId={device.id} /><Where /></MemoryRouter>);
  await screen.findByRole('heading', { name: 'Connection' });
  return view;
}
// Add device: the window the devices page opens over itself.
async function openAdd() {
  const view = render(<MemoryRouter initialEntries={['/desktop/devices']}><DeviceManagement action="add" onClose={vi.fn()} /></MemoryRouter>);
  await screen.findByLabelText('Device name');
  return view;
}
async function addDevice() {
  fireEvent.change(screen.getByLabelText('Device name'), { target: { value: 'New laptop' } });
  fireEvent.click(screen.getByRole('button', { name: 'Create token' }));
}
// The setup command is a setting of the device, not an item of its menu.
function copySetup(platform: 'unix' | 'windows' = 'unix') {
  fireEvent.click(screen.getByRole('button', { name: platform === 'unix' ? 'Copy setup for macOS/Linux' : 'Copy setup for Windows' }));
}
async function verify() {
  const dialog = await screen.findByRole('dialog', { name: 'Verify your identity' });
  fireEvent.change(within(dialog).getByLabelText('Password'), { target: { value: 'test-password' } });
  fireEvent.click(within(dialog).getByRole('button', { name: 'Verify' }));
}
function Where() { const location = useLocation(); return <p data-testid="where">{location.pathname + location.search}</p>; }

describe('device setup reauthentication', () => {
  it('offers Protect all devices for the owned device list and confirms before starting the batch', async () => {
    render(<MemoryRouter><UnprotectedDevicesButton devices={[device]} /></MemoryRouter>);
    await screen.findByRole('button', { name: 'Protect all devices' });
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
    await openAdd();
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
    expect(screen.queryByRole('dialog', { name: 'Verify your identity' })).toBeNull();
    // The window that asked now holds the new device's commands.
    expect(await screen.findByText(/Token created for/)).toBeVisible();
  });
  it.each(['unix', 'windows'] as const)('reverifies %s setup copying without replacing the pending secret', async (platform) => {
    vi.mocked(request).mockRejectedValueOnce(reauthenticationRequired());
    await openSettingsPane();
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
    await openSettingsPane();
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
    if (action === 'add') { await openAdd(); await addDevice(); } else { await openSettingsPane(); copySetup(); }
    const dialog = await screen.findByRole('dialog', { name: 'Verify your identity' });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Verify your identity' })).toBeNull());
    expect(mutation).toHaveBeenCalledTimes(1);
    expect(writeText).not.toHaveBeenCalled();
    expect(screen.queryByRole('alert')).toBeNull();
  });
  it('stops retrying if the relay still rejects Add device after successful verification', async () => {
    vi.mocked(createRelayDevice).mockRejectedValue(reauthenticationRequired());
    await openAdd(); await addDevice(); await verify();
    expect(await screen.findByRole('alert')).toHaveTextContent('Verify again');
    expect(createRelayDevice).toHaveBeenCalledTimes(2);
    expect(securityRequest).toHaveBeenCalledTimes(2);
    expect(prepareDeviceEnrollment).not.toHaveBeenCalled();
  });
  it.each([[403, 'forbidden'], [401, 'reauthentication_required'], [500, 'internal_error']] as const)('does not reverify unrelated %s %s failures', async (status, code) => {
    vi.mocked(createRelayDevice).mockRejectedValue(new ApiError(status, { code, message: 'Other failure' }));
    await openAdd(); await addDevice();
    expect(await screen.findByRole('alert')).toHaveTextContent('Other failure');
    expect(securityRequest).not.toHaveBeenCalled();
    expect(createRelayDevice).toHaveBeenCalledTimes(1);
  });
  it('does not retry a pending setup request when the page unmounts', async () => {
    vi.mocked(request).mockRejectedValueOnce(reauthenticationRequired());
    const view = await openSettingsPane(); copySetup();
    await screen.findByRole('dialog', { name: 'Verify your identity' });
    view.unmount();
    await act(async () => {});
    expect(request).toHaveBeenCalledTimes(1);
    expect(writeText).not.toHaveBeenCalled();
  });
  it('does not start verification when a rejected setup request arrives after unmount', async () => {
    let reject!: (error: unknown) => void;
    vi.mocked(request).mockReturnValueOnce(new Promise((_resolve, decline) => { reject = decline; }));
    const view = await openSettingsPane(); copySetup();
    await waitFor(() => expect(request).toHaveBeenCalledTimes(1));
    view.unmount();
    await act(async () => { reject(reauthenticationRequired()); });
    expect(securityRequest).not.toHaveBeenCalled();
    expect(writeText).not.toHaveBeenCalled();
  });
});

it('keeps the local command accessible for a pending enrollment on the device page', async () => {
  vi.mocked(useDeviceProtection).mockReturnValue('pending');
  render(<MemoryRouter><DeviceStatusLine device={device} portal={null} isolated={false} /></MemoryRouter>);
  // The status line keeps a short label; the command is one tap behind it.
  fireEvent.click(await screen.findByRole('button', {name:'Waiting for bind'}));
  fireEvent.click(screen.getByRole('button', {name:'Show local bind command'}));
  expect(screen.getByRole('dialog', {name:'Protect this device'})).toBeVisible();
});

describe('agent isolation', () => {
  it('reads and changes isolation on the device, in its settings and in its window', async () => {
    const online = { ...device, connected: true };
    vi.mocked(fetchRelayPortal).mockResolvedValue({ user: { id: 'test-user' } as never, devices: [online], sharedWithMe: [], sharedByMe: [] });
    vi.mocked(fetchAgentIsolation).mockResolvedValue({ device: false, workspaces: [] });
    vi.mocked(setDeviceIsolation).mockResolvedValue({ device: true, workspaces: [] });
    const view = await openSettingsPane();
    const toggle = await screen.findByRole('checkbox', { name: 'Isolate this device' });
    expect(fetchAgentIsolation).toHaveBeenCalledWith(device.id);
    expect(toggle).not.toBeChecked();
    fireEvent.click(toggle);
    await waitFor(() => expect(setDeviceIsolation).toHaveBeenCalledExactlyOnceWith(true, device.id));
    await waitFor(() => expect(toggle).toBeChecked());
    view.unmount();
    // The ⋯ menu's "Isolate…" opens the same control in a window.
    render(<MemoryRouter><DeviceManagement deviceId={device.id} action="isolate" onClose={vi.fn()} /></MemoryRouter>);
    const dialog = await screen.findByRole('dialog', { name: `Isolation · ${device.name}` });
    expect(await within(dialog).findByRole('checkbox', { name: 'Isolate this device' })).not.toBeChecked();
  });

  it('asks to update a device whose runtime predates isolation', async () => {
    const online = { ...device, connected: true };
    vi.mocked(fetchRelayPortal).mockResolvedValue({ user: { id: 'test-user' } as never, devices: [online], sharedWithMe: [], sharedByMe: [] });
    vi.mocked(fetchAgentIsolation).mockRejectedValue(new ApiError(404, { code: 'not_found', message: 'Not found' }));
    render(<MemoryRouter><DeviceManagement deviceId={device.id} action="isolate" onClose={vi.fn()} /></MemoryRouter>);
    // The toggle stays, disabled; its mark says why.
    const dialog = await screen.findByRole('dialog', { name: `Isolation · ${device.name}` });
    fireEvent.click(await within(dialog).findByRole('button', { name: 'Why it is unavailable' }));
    expect(screen.getByText('Update this device to change this.')).toBeTruthy();
    expect(within(dialog).getByRole('checkbox', { name: 'Isolate this device' })).toBeDisabled();
  });
});

describe('device shared for control', () => {
  const detail = 'Its agents cannot reach your other devices, because people you share with can control agents here.';
  const share = (id: string, extra: object) => ({
    id, ownerUserId: 'test-user', ownerUsername: 'me', targetUserId: `u-${id}`, targetUsername: id, deviceId: device.id, deviceName: device.name,
    threadId: 't1', threadTitle: 'Fix login', workspaceId: 'web', workspaceLabel: 'Web app', label: null, threadAccess: 'control', workspaceAccess: 'none',
    createdAt: '2026-10-01T00:00:00Z', revokedAt: null, expiresAt: null, lastAccessedAt: null, lastAccessedByUsername: null, accessEvents: [], ...extra,
  });
  const portal = () => ({
    user: { id: 'test-user' } as never, devices: [{ ...device, sharedControl: true }], sharedWithMe: [],
    sharedByMe: [share('alice', {}), share('waiting', { pending: true }), share('reader', { threadAccess: 'read' })] as never,
    grantsByMe: [share('carol', { scope: 'device', threadId: null, threadTitle: null, workspaceId: null, workspaceLabel: null, workspaceScope: 'all', workspaceIds: [], threadAccess: 'read', canCreateThreads: true })] as never,
  });
  it('marks the device page with a small icon that names each share causing it on hover and tap', async () => {
    render(<MemoryRouter><DeviceStatusLine device={{ ...device, sharedControl: true }} portal={portal()} isolated={false} /></MemoryRouter>);
    const mark = screen.getByRole('button', { name: 'Shared for control' });
    expect(mark).toHaveAttribute('title', detail);
    expect(mark).not.toHaveTextContent(/\S/);
    expect(screen.queryByRole('dialog')).toBeNull();
    fireEvent.click(mark);
    const popover = screen.getByRole('dialog', { name: 'Shared for control' });
    expect(popover).toHaveTextContent(detail);
    expect(popover).toHaveTextContent('Thread “Fix login” in Web app');
    expect(popover).toHaveTextContent('with alice · Collaborator');
    expect(popover).toHaveTextContent('The whole device');
    expect(popover).toHaveTextContent('with carol · View only, New threads');
    // Waiting for the safety-code check or read-only: not a reason.
    expect(popover).not.toHaveTextContent('waiting');
    expect(popover).not.toHaveTextContent('reader');
    expect(within(popover).getByRole('link', { name: 'Open Sharing' })).toHaveAttribute('href', '/desktop/devices?tab=shared&dir=out');
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
  });
  it('names the same shares beside the isolation control', async () => {
    vi.mocked(fetchRelayPortal).mockResolvedValue(portal());
    render(<MemoryRouter><DeviceManagement deviceId={device.id} action="isolate" onClose={vi.fn()} /></MemoryRouter>);
    const isolation = await screen.findByRole('dialog', { name: `Isolation · ${device.name}` });
    expect(isolation).toHaveTextContent(detail);
    expect(isolation).toHaveTextContent('with alice · Collaborator');
  });

  it.each([[{ sharedControl: false }], [{}]])('shows no mark for %j', (flag) => {
    render(<MemoryRouter><DeviceStatusLine device={{ ...device, ...flag }} portal={null} isolated={false} /></MemoryRouter>);
    expect(screen.queryByRole('button', { name: 'Shared for control' })).toBeNull();
  });
});

// Issue #115: a device's own settings are the Settings tab of its page.
describe('the device page settings: name, connection, isolation', () => {
  it('renames the device in place, from its settings', async () => {
    await openSettingsPane();
    fireEvent.click(screen.getByRole('button', { name: 'Rename' }));
    const dialog = screen.getByRole('dialog', { name: 'Rename device' });
    fireEvent.change(within(dialog).getByLabelText('Device name'), { target: { value: '  Kitchen Mac ' } });
    vi.mocked(fetchRelayPortal).mockResolvedValue({ user: { id: 'test-user' } as never, devices: [{ ...device, name: 'Kitchen Mac' }], sharedWithMe: [], sharedByMe: [] });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(request).toHaveBeenCalledExactlyOnceWith(`relay/devices/${device.id}`, { method: 'PATCH', body: JSON.stringify({ name: 'Kitchen Mac' }) }));
    expect(await screen.findByText('Kitchen Mac')).toBeVisible();
    expect(screen.queryByRole('dialog')).toBeNull();
    // The page did not change.
    expect(screen.getByTestId('where')).toHaveTextContent(`/desktop/devices/${device.id}?tab=settings`);
  });

  it('asks before dropping a changed name, and says so when the relay cannot rename', async () => {
    vi.mocked(request).mockRejectedValue(new ApiError(405, { code: 'unsupported', message: 'No' } as never));
    await openSettingsPane();
    fireEvent.click(screen.getByRole('button', { name: 'Rename' }));
    const dialog = screen.getByRole('dialog', { name: 'Rename device' });
    fireEvent.change(within(dialog).getByLabelText('Device name'), { target: { value: 'Other' } });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    const discard = screen.getByRole('dialog', { name: 'Discard unsaved changes?' });
    fireEvent.click(within(discard).getByRole('button', { name: 'Cancel' }));
    expect(within(dialog).getByLabelText('Device name')).toHaveValue('Other');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save' }));
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('This Relay does not support renaming devices yet.');
    // Nothing can be saved here, so closing asks nothing.
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    // From now on renaming is greyed, with the reason on its mark.
    const name = screen.getByRole('heading', { name: 'Name' }).parentElement!;
    expect(within(name).getByRole('button', { name: 'Rename' })).toBeDisabled();
    expect(within(name).getByRole('button', { name: 'Why it is unavailable' })).toBeVisible();
  });

  it('keeps the token and the setup commands, with a replacement that looks dangerous', async () => {
    await openSettingsPane();
    expect(screen.getByText(device.tokenPreview)).toBeVisible();
    const replace = screen.getByRole('button', { name: 'Replace device token' });
    expect(replace).toHaveClass('device-danger-button');
    expect(replace.querySelector('svg')).not.toBeNull();
    fireEvent.click(replace);
    const confirm = screen.getByRole('dialog', { name: 'Replace device token?' });
    fireEvent.click(within(confirm).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(request).not.toHaveBeenCalled();
  });

  it.each(['rename', 'share', 'isolate', 'remove'] as const)('opens the %s window alone, in place, for a card, the map or the device page', async (action) => {
    const onClose = vi.fn();
    render(<MemoryRouter initialEntries={['/desktop/devices']}><DeviceManagement deviceId={device.id} action={action} onClose={onClose} /><Where /></MemoryRouter>);
    const name = { rename: 'Rename device', share: `Share ${device.name}`, isolate: `Isolation · ${device.name}`, remove: 'Remove device' }[action];
    const dialog = await screen.findByRole('dialog', { name });
    // Only the window: the page keeps its own content, and its address.
    expect(screen.queryByRole('heading', { name: 'Connection' })).toBeNull();
    expect(screen.getByTestId('where')).toHaveTextContent('/desktop/devices');
    fireEvent.keyDown(dialog, { key: 'Escape' });
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });
});
