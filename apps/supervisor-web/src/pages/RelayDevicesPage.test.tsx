import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { RelayDeviceDto } from '@pockymoe/shared';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { ApiError, createRelayDevice, fetchAgentIsolation, fetchRelayPortal, request, setDeviceIsolation, setSelectedRelayDeviceId } from '../lib/api';
import { requireUnlock } from '../lib/accountVault';
import { accountPasswordProof } from '../lib/accountAuth';
import { prepareDeviceEnrollment } from '../lib/deviceEnrollment';
import { securityRequest } from '../lib/relaySecurity';
import { relaySupervisorCommand } from '../lib/deviceSetupCommands';
import { useDeviceProtection } from '../lib/useDeviceProtection';
import { RelayDevicesPage } from './RelayDevicesPage';
import { DeviceManagement } from '../components/DeviceManagement';
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
// The Supervisor block of a device's settings has its own test (RuntimeManagement.test.tsx).
vi.mock('../components/RuntimeManagement', () => ({ RuntimeManagement: ({ deviceId }: { deviceId: string }) => <p>Supervisor of {deviceId}</p> }));
// The line under a new device's command has its own test (DeviceSetupProgress.test.tsx).
vi.mock('../components/DeviceSetupProgress', () => ({ DeviceSetupProgress: () => null }));
vi.mock('../lib/remoteDeviceProtection', () => ({
  canSwitchLocalEnrollmentToRemote: vi.fn(async () => false), switchLocalEnrollmentToRemote: vi.fn(),
  protectDevices: vi.fn(), deviceProtectionProgress: vi.fn(), updateAndProtectDevice: vi.fn(),
  REMOTE_PROTECTION_CHANGED_EVENT: 'pockymoe-remote-protection-changed',
}));
vi.mock('../components/DeviceEncryptionStatus', () => ({ DeviceEncryptionStatus: () => null }));
vi.mock('../components/SignedShareDetails', () => ({ SignedShareDetails: () => null, OrphanDeviceGrants: () => null }));
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
function openMenu() {
  fireEvent.click(screen.getByRole('button', { name: `More actions for ${device.name}` }));
  return screen.getByRole('menu', { name: `Actions for ${device.name}` });
}
function openSettings() {
  fireEvent.click(within(openMenu()).getByRole('menuitem', { name: 'Settings…' }));
  return screen.getByRole('dialog', { name: `Device settings · ${device.name}` });
}
// The setup command is a setting of the device, not an item of its menu.
function copySetup(platform: 'unix' | 'windows' = 'unix') {
  fireEvent.click(within(openSettings()).getByRole('button', { name: platform === 'unix' ? 'Copy setup for macOS/Linux' : 'Copy setup for Windows' }));
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
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Verify your identity' })).toBeNull());
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
    fireEvent.click(within(openMenu()).getByRole('menuitem', { name: 'Isolate…' }));
    const dialog = await screen.findByRole('dialog', { name: `Isolation · ${device.name}` });
    const toggle = await within(dialog).findByRole('checkbox', { name: 'Isolate this device' });
    expect(fetchAgentIsolation).toHaveBeenCalledWith(device.id);
    expect(toggle).not.toBeChecked();
    fireEvent.click(toggle);
    await waitFor(() => expect(setDeviceIsolation).toHaveBeenCalledExactlyOnceWith(true, device.id));
    await waitFor(() => expect(toggle).toBeChecked());
    // The row's mark: a lock with the word, one tap from what it means.
    // (The open window hides the page behind it from assistive technology.)
    expect(within(screen.getByRole('article', { name: device.name, hidden: true })).getByRole('button', { name: 'Isolated', hidden: true })).toBeTruthy();
  });

  it('asks to update a device whose runtime predates isolation', async () => {
    const online = { ...device, connected: true };
    vi.mocked(fetchRelayPortal).mockResolvedValue({ user: { id: 'test-user' } as never, devices: [online], sharedWithMe: [], sharedByMe: [] });
    vi.mocked(fetchAgentIsolation).mockRejectedValue(new ApiError(404, { code: 'not_found', message: 'Not found' }));
    await openPage();
    fireEvent.click(within(openMenu()).getByRole('menuitem', { name: 'Isolate…' }));
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
  it('marks the row with a small icon that names each share causing it on hover and tap', async () => {
    vi.mocked(fetchRelayPortal).mockResolvedValue({
      user: { id: 'test-user' } as never, devices: [{ ...device, sharedControl: true }], sharedWithMe: [],
      sharedByMe: [share('alice', {}), share('waiting', { pending: true }), share('reader', { threadAccess: 'read' })] as never,
      grantsByMe: [share('carol', { scope: 'device', threadId: null, threadTitle: null, workspaceId: null, workspaceLabel: null, workspaceScope: 'all', workspaceIds: [], threadAccess: 'read', canCreateThreads: true })] as never,
    });
    await openPage();
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
    // The isolation dialog names the opposite direction too.
    fireEvent.click(within(openMenu()).getByRole('menuitem', { name: 'Isolate…' }));
    const isolation = await screen.findByRole('dialog', { name: `Isolation · ${device.name}` });
    expect(isolation).toHaveTextContent(detail);
    expect(isolation).toHaveTextContent('with alice · Collaborator');
  });

  it.each([[{ sharedControl: false }], [{}]])('shows no mark for %j', async (flag) => {
    vi.mocked(fetchRelayPortal).mockResolvedValue({ user: { id: 'test-user' } as never, devices: [{ ...device, ...flag }], sharedWithMe: [], sharedByMe: [] });
    await openPage();
    expect(screen.queryByRole('button', { name: 'Shared for control' })).toBeNull();
  });
});

// Issue #114: the phone's device rows are managed like the device cards and like workspaces.
describe('the device row is managed like a card and like a workspace', () => {
  function Where() { const location = useLocation(); return <p data-testid="where">{location.pathname}</p>; }
  async function openPageAt() {
    const view = render(<MemoryRouter initialEntries={['/relay-devices']}><RelayDevicesPage /><Where /></MemoryRouter>);
    await screen.findByText(device.name);
    return view;
  }
  it('lists the shared actions in order, at the page root, by the key or a right click', async () => {
    await openPage();
    const labels = ['Open', 'New thread', 'Device details', 'Rename', 'Share…', 'Isolate…', 'Settings…', 'Remove…'];
    const menu = openMenu();
    expect(menu.parentElement).toBe(document.body);
    expect(within(menu).getAllByRole('menuitem').map(item => item.textContent)).toEqual(labels);
    expect(within(menu).getByRole('menuitem', { name: 'Remove…' })).toHaveAttribute('data-tone', 'danger');
    // Offline: a new thread waits for the device, with the reason on a mark; the rest stays.
    expect(within(menu).getAllByRole('menuitem').filter(item => (item as HTMLButtonElement).disabled).map(item => item.textContent)).toEqual(['New thread']);
    expect(within(menu).getByRole('button', { name: 'Why it is unavailable' })).toBeVisible();
    fireEvent.keyDown(menu, { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    expect(fireEvent.contextMenu(screen.getByRole('article', { name: device.name }), { clientX: 30, clientY: 40 })).toBe(false);
    expect(within(screen.getByRole('menu', { name: `Actions for ${device.name}` })).getAllByRole('menuitem').map(item => item.textContent)).toEqual(labels);
  });

  it('opens an offline device instead of refusing, with the one connection mark on its row', async () => {
    await openPageAt();
    const row = screen.getByRole('article', { name: device.name });
    expect(row.querySelector('.admin-device-state.is-offline')).not.toBeNull();
    expect(row.querySelector('.device-presence-dot, .device-presence-label')).toBeNull();
    expect(within(row).queryByRole('button', { name: 'Connect' })).toBeNull();
    const open = within(row).getByRole('button', { name: 'Open' });
    expect(open).toBeEnabled();
    fireEvent.click(open);
    expect(setSelectedRelayDeviceId).toHaveBeenCalledWith(device.id);
    expect(screen.getByTestId('where')).toHaveTextContent(`/devices/${device.id}/workspaces`);
  });

  it('renames a device in place, where the phone had no entry', async () => {
    await openPageAt();
    fireEvent.click(within(openMenu()).getByRole('menuitem', { name: 'Rename' }));
    const dialog = screen.getByRole('dialog', { name: 'Rename device' });
    fireEvent.change(within(dialog).getByLabelText('Device name'), { target: { value: '  Kitchen Mac ' } });
    vi.mocked(fetchRelayPortal).mockResolvedValue({ user: { id: 'test-user' } as never, devices: [{ ...device, name: 'Kitchen Mac' }], sharedWithMe: [], sharedByMe: [] });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(request).toHaveBeenCalledExactlyOnceWith(`relay/devices/${device.id}`, { method: 'PATCH', body: JSON.stringify({ name: 'Kitchen Mac' }) }));
    expect(await screen.findByRole('article', { name: 'Kitchen Mac' })).toBeVisible();
    expect(screen.queryByRole('dialog')).toBeNull();
    // The page did not change.
    expect(screen.getByTestId('where')).toHaveTextContent('/relay-devices');
  });

  it('asks before dropping a changed name, and says so when the relay cannot rename', async () => {
    vi.mocked(request).mockRejectedValue(new ApiError(405, { code: 'unsupported', message: 'No' } as never));
    await openPage();
    fireEvent.click(within(openMenu()).getByRole('menuitem', { name: 'Rename' }));
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
    // From now on the action is greyed, with the reason on its mark.
    expect(within(openMenu()).getByRole('menuitem', { name: 'Rename' })).toBeDisabled();
  });

  it('keeps the token and the setup commands in the device\'s settings, with a replacement that looks dangerous', async () => {
    await openPage();
    expect(screen.getByRole('article', { name: device.name })).not.toHaveTextContent(device.tokenPreview);
    const settings = openSettings();
    expect(settings).toHaveTextContent(device.tokenPreview);
    expect(settings).toHaveTextContent(`Supervisor of ${device.id}`);
    const replace = within(settings).getByRole('button', { name: 'Replace device token' });
    expect(replace).toHaveClass('device-danger-button');
    expect(replace.querySelector('svg')).not.toBeNull();
    fireEvent.click(replace);
    // The confirmation opens in the window that asked, not behind it.
    const confirm = within(settings).getByRole('dialog', { name: 'Replace device token?' });
    fireEvent.click(within(confirm).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(within(settings).queryByRole('dialog')).toBeNull());
    expect(request).not.toHaveBeenCalled();
  });

  it('offers the first step in an empty list, as the workspace list does', async () => {
    vi.mocked(fetchRelayPortal).mockResolvedValue({ user: { id: 'test-user' } as never, devices: [], sharedWithMe: [], sharedByMe: [] });
    render(<MemoryRouter><RelayDevicesPage /></MemoryRouter>);
    const empty = (await screen.findByText('No devices yet. Add a device to create its permanent supervisor token.')).parentElement!;
    fireEvent.click(within(empty).getByRole('button', { name: 'Add device' }));
    expect(screen.getByLabelText('Device name')).toBeVisible();
  });

  it.each(['rename', 'share', 'isolate', 'settings', 'remove'] as const)('opens the %s window alone, in place, for a card or the map', async (action) => {
    const onClose = vi.fn();
    render(<MemoryRouter initialEntries={['/desktop/devices']}><DeviceManagement embedded deviceId={device.id} action={action} onClose={onClose} /><Where /></MemoryRouter>);
    const name = { rename: 'Rename device', share: `Share ${device.name}`, isolate: `Isolation · ${device.name}`, settings: `Device settings · ${device.name}`, remove: 'Remove device' }[action];
    const dialog = await screen.findByRole('dialog', { name });
    // Only the window: the page keeps its own list, and its address.
    expect(screen.queryByRole('article', { name: device.name })).toBeNull();
    expect(screen.getByTestId('where')).toHaveTextContent('/desktop/devices');
    fireEvent.keyDown(dialog, { key: 'Escape' });
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });
});
