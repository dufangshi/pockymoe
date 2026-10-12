import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { DeviceBindingPanel } from './DeviceBindingPanel';
import { prepareDeviceEnrollment } from '../lib/deviceEnrollment';
import { protectDevices } from '../lib/remoteDeviceProtection';
import type { RelayDeviceDto } from '@pockymoe/shared';
vi.mock('../lib/remoteDeviceProtection', () => ({
  protectDevices: vi.fn(), deviceProtectionProgress: vi.fn(async () => undefined), updateAndProtectDevice: vi.fn(),
  REMOTE_PROTECTION_CHANGED_EVENT: 'pockymoe-remote-protection-changed',
}));
vi.mock('../lib/deviceEnrollment', () => ({
  prepareDeviceEnrollment: vi.fn(),
}));
beforeEach(() => {
  vi.clearAllMocks();
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
it('offers remote protection beside the local command and retains local repair for bound devices', async () => {
  const device = { id: 'offline', connected: false, ownerUserId: 'u', name: 'Laptop' } as RelayDeviceDto;
  const view = render(<DeviceBindingPanel deviceId={device.id} device={device} />);
  expect(await screen.findByRole('button', { name: 'Copy bind command' })).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Protect this device remotely' }));
  fireEvent.click(screen.getByRole('button', { name: 'Protect devices' }));
  await waitFor(() => expect(protectDevices).toHaveBeenCalledExactlyOnceWith([device]));
  view.unmount();
  render(<DeviceBindingPanel deviceId={device.id} device={device} repair />);
  expect(await screen.findByRole('button', { name: 'Copy bind command' })).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Protect this device remotely' })).toBeNull();
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
