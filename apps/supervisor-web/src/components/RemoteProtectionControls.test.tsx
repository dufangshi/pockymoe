import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import type { RelayDeviceDto } from '@pockymoe/shared';
import { ProtectDevicesButton } from './RemoteProtectionControls';
import { protectDevices } from '../lib/remoteDeviceProtection';
vi.mock('../lib/remoteDeviceProtection', () => ({
  canSwitchLocalEnrollmentToRemote: vi.fn(async () => false),
  switchLocalEnrollmentToRemote: vi.fn(),
  deviceProtectionProgress: vi.fn(),
  protectDevices: vi.fn(),
  updateAndProtectDevice: vi.fn(),
  REMOTE_PROTECTION_CHANGED_EVENT: 'pockymoe-remote-protection-changed',
}));
const device: RelayDeviceDto = {
  id: 'test-device',
  name: 'Laptop',
  ownerUserId: 'owner',
  connected: true,
  token: null,
  tokenPreview: '',
  connectedAt: null,
  lastHeartbeatAt: null,
  createdAt: '',
};
beforeEach(() => {
  vi.resetAllMocks();
  setLocale('en');
  vi.mocked(protectDevices).mockResolvedValue(undefined);
});
describe('remote protection controls', () => {
  it.each(['en', 'zh-CN'] as const)(
    'shows the trade-off and local alternative before %s batch approval',
    async (locale) => {
      setLocale(locale);
      render(<ProtectDevicesButton devices={[device]} all />);
      fireEvent.click(
        screen.getByRole('button', {
          name: translate('devices.protectAllDevices'),
        }),
      );
      const dialog = screen.getByRole('dialog', {
        name: translate('devices.protectAllDevices'),
      });
      expect(
        within(dialog).getByText(translate('devices.remoteProtectionTradeoff')),
      ).toBeVisible();
      expect(
        within(dialog).getByText(
          translate('devices.remoteProtectionLocalOption'),
        ),
      ).toBeVisible();
      expect(
        within(dialog).getByText(
          translate('devices.remoteProtectionUpdatesHelp'),
        ),
      ).toBeVisible();
      expect(protectDevices).not.toHaveBeenCalled();
      fireEvent.click(
        within(dialog).getByRole('button', {
          name: translate('devices.confirmRemoteProtection'),
        }),
      );
      await waitFor(() =>
        expect(protectDevices).toHaveBeenCalledExactlyOnceWith([device]),
      );
      await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    },
  );
  it('offers the per-device action for offline devices too', async () => {
    const offline = { ...device, connected: false };
    render(<ProtectDevicesButton devices={[offline]} />);
    fireEvent.click(
      screen.getByRole('button', { name: 'Protect this device remotely' }),
    );
    fireEvent.click(
      within(screen.getByRole('dialog')).getByRole('button', {
        name: 'Protect devices',
      }),
    );
    await waitFor(() =>
      expect(protectDevices).toHaveBeenCalledExactlyOnceWith([offline]),
    );
  });
});
