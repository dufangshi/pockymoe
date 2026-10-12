import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import type { RelayDeviceDto } from '@pockymoe/shared';
import {
  ProtectDevicesButton,
  RemoteProtectionStatus,
} from './RemoteProtectionControls';
import {
  deviceProtectionProgress,
  protectDevices,
  updateAndProtectDevice,
  REMOTE_PROTECTION_CHANGED_EVENT,
} from '../lib/remoteDeviceProtection';
vi.mock('../lib/remoteDeviceProtection', () => ({
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
  vi.mocked(updateAndProtectDevice).mockResolvedValue(undefined);
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
  it('offers an explicit update action before updating an older device', async () => {
    vi.mocked(deviceProtectionProgress).mockResolvedValue({
      phase: 'update-required',
    });
    render(<RemoteProtectionStatus device={device} />);
    expect(await screen.findByRole('status')).toHaveTextContent(
      'Update this device before protecting it remotely.',
    );
    expect(updateAndProtectDevice).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Update and protect' }));
    await waitFor(() =>
      expect(updateAndProtectDevice).toHaveBeenCalledExactlyOnceWith(device.id),
    );
  });
  it('shows account mismatch with the required local reset command and keeps retry disabled offline', async () => {
    vi.mocked(deviceProtectionProgress).mockResolvedValue({
      phase: 'error',
      error: 'account-mismatch',
    });
    render(<RemoteProtectionStatus device={{ ...device, connected: false }} />);
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'another account key',
    );
    expect(screen.getByRole('alert')).toHaveTextContent(
      'pockymoe bind --reset',
    );
    expect(
      screen.getByRole('button', { name: 'Retry enrollment' }),
    ).toBeDisabled();
  });
  it.each(['certificate-missing', 'identity-changed'] as const)(
    'shows a pinned-device %s error without reset advice or remote retry',
    async (phase) => {
      vi.mocked(deviceProtectionProgress).mockResolvedValue({ phase });
      render(<RemoteProtectionStatus device={device} />);
      const alert = await screen.findByRole('alert');
      expect(alert).toHaveTextContent(
        translate(
          phase === 'certificate-missing'
            ? 'devices.certificateMissingHelp'
            : 'auth.accountDeviceIdentityChanged',
        ),
      );
      expect(alert).not.toHaveTextContent('pockymoe bind --reset');
      expect(screen.queryByRole('button')).toBeNull();
    },
  );
  it.each(['en', 'zh-CN'] as const)(
    'explains a pending local command in %s without offering remote retry',
    async (locale) => {
      setLocale(locale);
      vi.mocked(deviceProtectionProgress).mockResolvedValue({
        phase: 'local-command-pending',
      });
      render(<RemoteProtectionStatus device={device} />);
      expect(await screen.findByRole('status')).toHaveTextContent(
        translate('devices.localCommandPending'),
      );
      expect(screen.queryByRole('button')).toBeNull();
      expect(protectDevices).not.toHaveBeenCalled();
    },
  );
  it('refreshes per-device progress as durable intent advances', async () => {
    vi.mocked(deviceProtectionProgress).mockResolvedValue({ phase: 'queued' });
    render(<RemoteProtectionStatus device={device} />);
    expect(await screen.findByRole('status')).toHaveTextContent(
      'waiting for the device to connect',
    );
    vi.mocked(deviceProtectionProgress).mockResolvedValue({
      phase: 'certifying',
    });
    act(() => window.dispatchEvent(new Event(REMOTE_PROTECTION_CHANGED_EVENT)));
    await waitFor(() =>
      expect(screen.getByRole('status')).toHaveTextContent(
        'Verifying enrollment',
      ),
    );
  });
});
