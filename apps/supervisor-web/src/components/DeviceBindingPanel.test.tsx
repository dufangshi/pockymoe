import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { DeviceBindingPanel } from './DeviceBindingPanel';
import { prepareDeviceEnrollment } from '../lib/deviceEnrollment';
vi.mock('../lib/deviceEnrollment', () => ({
  prepareDeviceEnrollment: vi.fn(),
}));
beforeEach(() => {
  vi.clearAllMocks();
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
it('shows and copies a local command without requesting the offline device', async () => {
  render(<DeviceBindingPanel deviceId="offline" />);
  fireEvent.click(
    await screen.findByRole('button', { name: 'Copy bind command' }),
  );
  await waitFor(() =>
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
      expect.stringContaining(
        'POCKYMOE_ENROLL=secret pockymoe bind --account-key public --web-origin',
      ),
    ),
  );
  expect(prepareDeviceEnrollment).toHaveBeenCalledWith('offline');
  expect(await screen.findByText('Setup command copied.')).toBeVisible();
});
