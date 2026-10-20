import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import type { RelayDeviceDto } from '@pockymoe/shared';
import { DeviceProtectionBadge } from './DeviceProtectionBadge';
import {
  canSwitchLocalEnrollmentToRemote,
  clearResolvedAccountMismatch,
  deviceProtectionProgress,
  protectDevices,
  switchLocalEnrollmentToRemote,
  updateAndProtectDevice,
  REMOTE_PROTECTION_CHANGED_EVENT,
} from '../lib/remoteDeviceProtection';

vi.mock('../lib/remoteDeviceProtection', () => ({
  canSwitchLocalEnrollmentToRemote: vi.fn(async () => false),
  clearResolvedAccountMismatch: vi.fn(async () => false),
  switchLocalEnrollmentToRemote: vi.fn(),
  deviceProtectionProgress: vi.fn(),
  protectDevices: vi.fn(),
  updateAndProtectDevice: vi.fn(),
  REMOTE_PROTECTION_CHANGED_EVENT: 'pockymoe-remote-protection-changed',
}));
vi.mock('../lib/accountVault', () => ({ currentAccount: async () => ({ accountKeyId: 'Jj39GRPC2V9Pe1hV2KdIojvRxmQ' }) }));
vi.mock('../lib/finishPendingProtection', () => ({ finishPendingProtection: vi.fn(async () => true) }));
import { finishPendingProtection } from '../lib/finishPendingProtection';
vi.mock('./DeviceBindingPanel', () => ({
  DeviceBindingPanel: ({ repair }: { repair?: boolean }) => <p>binding panel{repair ? ' (repair)' : ''}</p>,
}));

const device: RelayDeviceDto = {
  id: 'test-device', name: 'Laptop', ownerUserId: 'owner', connected: true, token: null,
  tokenPreview: '', connectedAt: null, lastHeartbeatAt: null, createdAt: '',
};
beforeEach(() => {
  vi.resetAllMocks();
  setLocale('en');
  vi.mocked(deviceProtectionProgress).mockResolvedValue(undefined);
  vi.mocked(canSwitchLocalEnrollmentToRemote).mockResolvedValue(false);
  vi.mocked(clearResolvedAccountMismatch).mockResolvedValue(false);
  vi.mocked(protectDevices).mockResolvedValue(undefined);
  vi.mocked(switchLocalEnrollmentToRemote).mockResolvedValue(undefined);
  vi.mocked(updateAndProtectDevice).mockResolvedValue(undefined);
});
async function openBadge(name: string) {
  fireEvent.click(await screen.findByRole('button', { name }));
  return screen.getByRole('dialog', { name });
}

it('keeps the row to a short label and puts the explanation and help link behind it', async () => {
  render(<DeviceProtectionBadge device={device} protection="not-protected" />);
  const panel = await openBadge(translate('devices.state.notProtected'));
  expect(within(panel).getByText(translate('devices.detail.notProtected'))).toBeVisible();
  expect(within(panel).getByRole('link', { name: /Learn more/ })).toHaveAttribute('href', '/help/devices#not-protected');
  fireEvent.click(within(panel).getByRole('button', { name: translate('devices.protectDevice') }));
  expect(await screen.findByText('binding panel')).toBeVisible();
});

it('shows a protected device as an icon only', async () => {
  render(<DeviceProtectionBadge device={device} protection="protected" />);
  const badge = await screen.findByRole('button', { name: translate('devices.state.protected') });
  expect(badge).not.toHaveTextContent(translate('devices.state.protected'));
});

it('offers an explicit update action before updating an older device', async () => {
  vi.mocked(deviceProtectionProgress).mockResolvedValue({ phase: 'update-required' });
  render(<DeviceProtectionBadge device={device} protection="pending" />);
  const panel = await openBadge('Update needed');
  expect(updateAndProtectDevice).not.toHaveBeenCalled();
  fireEvent.click(within(panel).getByRole('button', { name: 'Update and protect' }));
  await waitFor(() => expect(updateAndProtectDevice).toHaveBeenCalledExactlyOnceWith(device.id));
});

it('explains an account mismatch with the account ID to compare, a fix link, and retry disabled offline', async () => {
  vi.mocked(deviceProtectionProgress).mockResolvedValue({ phase: 'error', error: 'account-mismatch' });
  render(<DeviceProtectionBadge device={{ ...device, connected: false }} protection="pending" />);
  const panel = await openBadge('Bound to another account');
  expect(await within(panel).findByText(translate('devices.yourAccountId', { value1: 'Jj39GRPC2V9P' }))).toBeVisible();
  expect(within(panel).getByRole('link', { name: /How to fix/ })).toHaveAttribute('href', '/help/devices#other-account');
  expect(within(panel).getByRole('button', { name: 'Retry' })).toBeDisabled();
  expect(clearResolvedAccountMismatch).not.toHaveBeenCalled();
});

it('clears a resolved mismatch once the device is online', async () => {
  vi.mocked(deviceProtectionProgress).mockResolvedValue({ phase: 'error', error: 'account-mismatch' });
  render(<DeviceProtectionBadge device={device} protection="pending" />);
  await waitFor(() => expect(clearResolvedAccountMismatch).toHaveBeenCalledExactlyOnceWith(device));
});

it.each([
  ['certificate-missing', 'Needs repair'],
  ['identity-changed', 'Identity changed'],
] as const)('shows a pinned-device %s error without reset advice or remote retry', async (phase, name) => {
  vi.mocked(deviceProtectionProgress).mockResolvedValue({ phase });
  render(<DeviceProtectionBadge device={device} protection="unknown" />);
  const panel = await openBadge(name);
  expect(panel).not.toHaveTextContent('pockymoe bind --reset');
  expect(within(panel).queryByRole('button', { name: /Retry|Update/ })).toBeNull();
  // Repair is the local, MAC-gated path; an identity change offers no action.
  expect(within(panel).queryAllByRole('button').map(b => b.textContent)).toEqual(
    phase === 'certificate-missing' ? [translate('devices.repairDevice')] : [],
  );
});

it.each(['en', 'zh-CN'] as const)('explains a pending local command in %s, offering to finish it rather than a remote retry', async (locale) => {
  setLocale(locale);
  vi.mocked(deviceProtectionProgress).mockResolvedValue({ phase: 'local-command-pending' });
  render(<DeviceProtectionBadge device={device} protection="pending" />);
  const panel = await openBadge(translate('devices.state.waitingBind'));
  expect(within(panel).getByText(translate('devices.detail.waitingBind'))).toBeVisible();
  // Nothing finishes protection in the background: the reader finishes it here.
  expect(within(panel).getAllByRole('button').map(b => b.textContent)).toEqual([translate('devices.finishProtection'), translate('devices.showLocalBindCommand')]);
  expect(protectDevices).not.toHaveBeenCalled();
});

it('refreshes per-device progress as durable intent advances', async () => {
  vi.mocked(deviceProtectionProgress).mockResolvedValue({ phase: 'queued' });
  render(<DeviceProtectionBadge device={device} protection="pending" />);
  expect(await screen.findByRole('button', { name: 'Waiting for device' })).toBeVisible();
  vi.mocked(deviceProtectionProgress).mockResolvedValue({ phase: 'certifying' });
  act(() => window.dispatchEvent(new Event(REMOTE_PROTECTION_CHANGED_EVENT)));
  expect(await screen.findByRole('button', { name: 'Protecting' })).toBeVisible();
});

it.each(['en', 'zh-CN'] as const)('confirms the trade-off before discarding a local enrollment in %s', async (locale) => {
  setLocale(locale);
  vi.mocked(deviceProtectionProgress).mockResolvedValue({ phase: 'local-command-pending' });
  vi.mocked(canSwitchLocalEnrollmentToRemote).mockResolvedValue(true);
  render(<DeviceProtectionBadge device={device} protection="pending" />);
  const panel = await openBadge(translate('devices.state.waitingBind'));
  fireEvent.click(await within(panel).findByRole('button', { name: translate('devices.useOneClickProtectionInstead') }));
  const confirm = screen.getByRole('dialog', { name: translate('devices.useOneClickProtectionInstead') });
  expect(within(confirm).getByText(translate('devices.replaceLocalEnrollmentHelp'))).toBeVisible();
  expect(switchLocalEnrollmentToRemote).not.toHaveBeenCalled();
  fireEvent.click(within(confirm).getByRole('button', { name: translate('devices.confirmRemoteProtection') }));
  await waitFor(() => expect(switchLocalEnrollmentToRemote).toHaveBeenCalledExactlyOnceWith(device));
  expect(protectDevices).not.toHaveBeenCalled();
});

it('hides switching offline and refreshes eligibility on reconnect', async () => {
  vi.mocked(deviceProtectionProgress).mockResolvedValue({ phase: 'local-command-pending' });
  vi.mocked(canSwitchLocalEnrollmentToRemote).mockResolvedValue(true);
  const view = render(<DeviceProtectionBadge device={device} protection="pending" />);
  const panel = await openBadge('Waiting for bind');
  expect(await within(panel).findByRole('button', { name: 'Use one-click protection instead' })).toBeVisible();
  view.rerender(<DeviceProtectionBadge device={{ ...device, connected: false }} protection="pending" />);
  expect(screen.queryByRole('button', { name: 'Use one-click protection instead' })).toBeNull();
  view.rerender(<DeviceProtectionBadge device={device} protection="pending" />);
  expect(await screen.findByRole('button', { name: 'Use one-click protection instead' })).toBeVisible();
});

it('does not offer switching while remote protection is queued, and finishes it only when asked', async () => {
  vi.mocked(deviceProtectionProgress).mockResolvedValue({ phase: 'queued' });
  vi.mocked(canSwitchLocalEnrollmentToRemote).mockResolvedValue(true);
  render(<DeviceProtectionBadge device={device} protection="pending" />);
  const panel = await openBadge('Waiting for device');
  expect(canSwitchLocalEnrollmentToRemote).not.toHaveBeenCalled();
  expect(within(panel).getAllByRole('button').map(b => b.textContent)).toEqual(['Finish protection']);
  expect(finishPendingProtection).not.toHaveBeenCalled();
  await act(async () => fireEvent.click(within(panel).getByRole('button', { name: 'Finish protection' })));
  expect(finishPendingProtection).toHaveBeenCalledWith(device.id);
});
