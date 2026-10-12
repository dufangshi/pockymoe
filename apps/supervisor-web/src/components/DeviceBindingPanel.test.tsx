import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { DeviceBindingPanel } from './DeviceBindingPanel';
import { prepareDeviceEnrollment } from '../lib/deviceEnrollment';
import {
  protectDevices,
  deviceProtectionProgress,
  REMOTE_PROTECTION_CHANGED_EVENT,
} from '../lib/remoteDeviceProtection';
import type { RelayDeviceDto } from '@pockymoe/shared';
vi.mock('../lib/remoteDeviceProtection', () => ({
  protectDevices: vi.fn(),
  deviceProtectionProgress: vi.fn(async () => undefined),
  updateAndProtectDevice: vi.fn(),
  REMOTE_PROTECTION_CHANGED_EVENT: 'pockymoe-remote-protection-changed',
}));
vi.mock('../lib/deviceEnrollment', () => ({
  prepareDeviceEnrollment: vi.fn(),
}));
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(deviceProtectionProgress).mockResolvedValue(undefined);
  vi.mocked(protectDevices).mockResolvedValue(undefined);
  vi.mocked(prepareDeviceEnrollment).mockResolvedValue({
    deviceId: 'offline',
    userId: 'u',
    accountKeyId: 'a',
    accountPublicKey: 'public',
    enrollSecret: 'secret',
    createdAt: Date.now(),
  });
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText: vi.fn().mockResolvedValue(undefined) },
  });
});
const device = {
  id: 'offline',
  connected: false,
  ownerUserId: 'u',
  name: 'Laptop',
} as RelayDeviceDto;
it('offers the remote or local choice without creating an enrollment just by opening the panel', async () => {
  const view = render(
    <DeviceBindingPanel deviceId={device.id} device={device} />,
  );
  const remote = await screen.findByRole('button', {
    name: 'Protect this device remotely',
  });
  expect(
    screen.getByRole('button', { name: 'Show local bind command' }),
  ).toBeVisible();
  expect(prepareDeviceEnrollment).not.toHaveBeenCalled();
  expect(
    screen.queryByRole('button', { name: 'Copy bind command' }),
  ).toBeNull();
  fireEvent.click(remote);
  fireEvent.click(screen.getByRole('button', { name: 'Protect devices' }));
  await waitFor(() =>
    expect(protectDevices).toHaveBeenCalledExactlyOnceWith([device]),
  );
  view.unmount();
  render(<DeviceBindingPanel deviceId={device.id} device={device} repair />);
  expect(
    await screen.findByRole('button', { name: 'Copy bind command' }),
  ).toBeVisible();
  expect(
    screen.queryByRole('button', { name: 'Protect this device remotely' }),
  ).toBeNull();
});
it('hides remote binding after the user chooses a local command', async () => {
  render(<DeviceBindingPanel deviceId={device.id} device={device} />);
  fireEvent.click(
    await screen.findByRole('button', { name: 'Show local bind command' }),
  );
  expect(
    await screen.findByRole('button', { name: 'Copy bind command' }),
  ).toBeVisible();
  expect(prepareDeviceEnrollment).toHaveBeenCalledExactlyOnceWith(device.id);
  expect(
    screen.queryByRole('button', { name: 'Protect this device remotely' }),
  ).toBeNull();
});
it('reopens an existing local enrollment with its command and no remote alternative', async () => {
  vi.mocked(deviceProtectionProgress).mockResolvedValue({
    phase: 'local-command-pending',
  });
  render(<DeviceBindingPanel deviceId={device.id} device={device} />);
  expect(
    await screen.findByRole('button', { name: 'Copy bind command' }),
  ).toBeVisible();
  expect(
    await screen.findByText(/Waiting for the local bind command/),
  ).toBeVisible();
  expect(
    screen.queryByRole('button', { name: 'Protect this device remotely' }),
  ).toBeNull();
});
it('keeps a remote enrollment exclusive when a progress event arrives from another view', async () => {
  render(<DeviceBindingPanel deviceId={device.id} device={device} />);
  expect(
    await screen.findByRole('button', { name: 'Show local bind command' }),
  ).toBeVisible();
  vi.mocked(deviceProtectionProgress).mockResolvedValue({ phase: 'queued' });
  act(() => window.dispatchEvent(new Event(REMOTE_PROTECTION_CHANGED_EVENT)));
  await waitFor(() =>
    expect(
      screen.queryByRole('button', { name: 'Show local bind command' }),
    ).toBeNull(),
  );
  expect(prepareDeviceEnrollment).not.toHaveBeenCalled();
  expect(
    screen.queryByRole('button', { name: 'Copy bind command' }),
  ).toBeNull();
});
it('shows and copies a local command without requesting the offline device', async () => {
  render(<DeviceBindingPanel deviceId="offline" />);
  fireEvent.click(
    await screen.findByRole('button', { name: 'Copy bind command' }),
  );
  await waitFor(() =>
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
      expect.stringContaining(
        "POCKYMOE_ENROLL_SECRET='secret' pockymoe bind --account-key 'public' --web-origin",
      ),
    ),
  );
  expect(prepareDeviceEnrollment).toHaveBeenCalledWith('offline');
  expect(await screen.findByText('Setup command copied.')).toBeVisible();
});
it('requests a fresh enrollment for missing-certificate repair and offers both platform commands', async () => {
  render(<DeviceBindingPanel deviceId="offline" repair />);
  const copy = await screen.findByRole('button', { name: 'Copy bind command' });
  expect(prepareDeviceEnrollment).toHaveBeenCalledWith('offline', {
    fresh: true,
  });
  fireEvent.click(screen.getByRole('button', { name: 'Windows' }));
  fireEvent.click(copy);
  await waitFor(() =>
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
      expect.stringContaining("$env:POCKYMOE_ENROLL_SECRET = 'secret'"),
    ),
  );
  expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
    expect.stringContaining('Remove-Item Env:POCKYMOE_ENROLL_SECRET'),
  );
});
