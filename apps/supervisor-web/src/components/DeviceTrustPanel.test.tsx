import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import { DeviceTrustPanel } from './DeviceTrustPanel';
import { deviceTrustRequest, type DeviceTrust } from '../lib/deviceTrust';
import {
  currentAccount,
  boundDevices,
  requireUnlock,
  verifyDeviceCertificate,
} from '../lib/accountVault';
import {
  listPendingEnrollments,
  prepareDeviceEnrollment,
} from '../lib/deviceEnrollment';
import vectors from '../lib/fixtures/deviceEnrollmentVectors.json';
vi.mock('../lib/deviceTrust', () => ({ deviceTrustRequest: vi.fn() }));
vi.mock('../lib/accountVault', () => ({
  currentAccount: vi.fn(),
  boundDevices: vi.fn(),
  requireUnlock: vi.fn(),
  verifyDeviceCertificate: vi.fn(),
}));
vi.mock('../lib/deviceEnrollment', async (original) => ({
  ...(await original<typeof import('../lib/deviceEnrollment')>()),
  listPendingEnrollments: vi.fn(),
  prepareDeviceEnrollment: vi.fn(),
}));
let trust: DeviceTrust;
beforeEach(() => {
  setLocale('en');
  vi.clearAllMocks();
  trust = {
    deviceId: 'device-1',
    bound: true,
    strict: true,
    accountKeyId: vectors.account.accountKeyId,
    certificate: { ...vectors.deviceCertificate, version: 1 },
    revocations: 0,
    browsers: [
      {
        browserId: 'browser-long-id-123',
        role: 'owner',
        userId: null,
        registeredAt: 1760000000000,
        lastSeenAt: 1760000100000,
        current: true,
      },
    ],
  };
  vi.mocked(deviceTrustRequest).mockImplementation(async () => trust as never);
  vi.mocked(currentAccount).mockResolvedValue(vectors.account);
  vi.mocked(boundDevices).mockResolvedValue(['device-1']);
  vi.mocked(listPendingEnrollments).mockResolvedValue([]);
  vi.mocked(verifyDeviceCertificate).mockResolvedValue(true);
  vi.mocked(prepareDeviceEnrollment).mockResolvedValue({
    deviceId: 'device-1',
    userId: vectors.account.userId,
    accountKeyId: vectors.account.accountKeyId,
    accountPublicKey: vectors.account.accountPublicKey,
    enrollSecret: 'pending-secret',
    createdAt: Date.now(),
  });
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText: vi.fn().mockResolvedValue(undefined) },
  });
});
describe('account bound device trust panel', () => {
  it.each(['en', 'zh-CN'] as const)(
    'shows verified protection, account fingerprint and registered browsers in %s',
    async (locale) => {
      setLocale(locale);
      render(<DeviceTrustPanel deviceId="device-1" />);
      expect(
        await screen.findByText(translate('devices.protected')),
      ).toBeVisible();
      expect(
        screen.getByText(vectors.account.accountKeyId.slice(0, 12)),
      ).toBeVisible();
      expect(screen.getByTitle('browser-long-id-123')).toHaveTextContent(
        'browser-long',
      );
      expect(screen.getByText(translate('auth.ownerCurrent'))).toBeVisible();
      expect(
        screen.getByText(
          translate('devices.browserRegisteredAt', {
            value1: new Date(1760000000000).toLocaleString(locale),
          }),
        ),
      ).toBeVisible();
      expect(
        screen.getByText(
          translate('devices.browserLastSeenAt', {
            value1: new Date(1760000100000).toLocaleString(locale),
          }),
        ),
      ).toBeVisible();
      expect(screen.queryByText(/passkey|通行密钥/)).toBeNull();
      expect(requireUnlock).not.toHaveBeenCalled();
    },
  );
  it('never uses the unsigned bound hint as proof', async () => {
    vi.mocked(verifyDeviceCertificate).mockResolvedValue(false);
    render(<DeviceTrustPanel deviceId="device-1" />);
    expect(await screen.findByRole('alert')).toHaveTextContent(
      translate('devices.certificateMissing'),
    );
    expect(
      screen.getByText(translate('devices.certificateMissingHelp')),
    ).toBeVisible();
  });
  it.each(['en', 'zh-CN'] as const)(
    'prepares and copies a local bind command for an unbound device in %s',
    async (locale) => {
      setLocale(locale);
      trust.bound = false;
      trust.strict = false;
      trust.certificate = null;
      trust.accountKeyId = null;
      vi.mocked(boundDevices).mockResolvedValue([]);
      render(<DeviceTrustPanel deviceId="device-1" />);
      expect(
        await screen.findByText(translate('devices.notProtected')),
      ).toBeVisible();
      fireEvent.click(
        screen.getByRole('button', {
          name: translate('devices.protectDevice'),
        }),
      );
      const copy = await screen.findByRole('button', {
        name: translate('devices.copyBindCommand'),
      });
      expect(prepareDeviceEnrollment).toHaveBeenCalledWith('device-1');
      fireEvent.click(copy);
      await waitFor(() =>
        expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
          expect.stringContaining('pockymoe bind --account-key'),
        ),
      );
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
        expect.stringMatching(/^POCKYMOE_ENROLL_SECRET='pending-secret' pockymoe bind --account-key '[A-Za-z0-9_-]+' --web-origin '/),
      );
      expect(
        screen.getByText(translate('devices.protectDeviceHelp')),
      ).toBeVisible();
    },
  );
  it('ignores a late response for the previous device', async () => {
    let resolve!: (v: DeviceTrust) => void;
    vi.mocked(deviceTrustRequest).mockReturnValueOnce(
      new Promise((r) => {
        resolve = r;
      }),
    );
    const { rerender } = render(<DeviceTrustPanel deviceId="old-device" />);
    rerender(<DeviceTrustPanel deviceId="device-1" />);
    await screen.findByText(translate('devices.protected'));
    resolve({ ...trust, accountKeyId: 'old-fingerprint' });
    await waitFor(() => expect(screen.queryByText('old-fingerpri')).toBeNull());
  });
});
