import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import { DeviceTrustPanel } from './DeviceTrustPanel';
import { deviceTrustRequest, type DeviceTrust } from '../lib/deviceTrust';
import {
  assertOwner,
  clearOwnerSession,
  currentOwnerSession,
  fetchOwnerCredentials,
  OwnerTrustError,
} from '../lib/ownerSession';
import vectors from '../lib/fixtures/ownerTrustVectors.json';
import { type OwnerCredential } from '../lib/ownerTrustCrypto';
vi.mock('../lib/deviceTrust', () => ({ deviceTrustRequest: vi.fn() }));
vi.mock('../lib/ownerSession', async (importOriginal) => ({
  ...await importOriginal<typeof import('../lib/ownerSession')>(),
  assertOwner: vi.fn(),
  clearOwnerSession: vi.fn(),
  currentOwnerSession: vi.fn(),
  fetchOwnerCredentials: vi.fn(),
}));
vi.mock('../lib/relaySecurity', () => ({
  securityError: (e: unknown) => (e instanceof Error ? e.message : 'Failed'),
}));
vi.mock('../lib/ownerDeviceTrust', () => ({
  appendDeviceCertificate: vi.fn().mockResolvedValue(undefined),
}));
vi.mock('./OwnerUnlockPrompt', () => ({ OwnerUnlockButton: () => null }));
const credential = vectors.credentialRecord as OwnerCredential;
const second = { ...credential, credentialId: 'c2', name: 'Second passkey' };
let trust: DeviceTrust;
beforeEach(() => {
  setLocale('en');
  vi.clearAllMocks();
  trust = {
    deviceId: 'd',
    paired: true,
    strict: true,
    credentials: [credential],
    certificate: {
      ...vectors.deviceCertificate.certificate,
      deviceId: 'd',
      version: 1,
    },
    sessions: [
      {
        sessionId: 's',
        credentialId: credential.credentialId,
        expiresAt: Date.now() + 10000,
        current: true,
      },
    ],
  };
  vi.mocked(deviceTrustRequest).mockImplementation(
    async (_deviceId, path = '') => (path ? ({} as never) : (trust as never)),
  );
  vi.mocked(fetchOwnerCredentials).mockResolvedValue([credential, second]);
  vi.mocked(currentOwnerSession).mockResolvedValue({
    sessionId: 's',
    registration: { assertion: { credentialId: credential.credentialId } },
  } as never);
  vi.mocked(assertOwner).mockResolvedValue({
    credential: second,
    assertion: vectors.session.registration.assertion,
  });
});
describe('device trust controls', () => {
  it.each(['en', 'zh-CN'] as const)('shows the paired-origin error in the %s add-passkey flow and does not certify the rejected credential', async (locale) => {
    setLocale(locale);
    vi.mocked(deviceTrustRequest).mockImplementation(async (_id, path = '') => {
      if (path === 'credentials') throw new OwnerTrustError('owner_origin_mismatch');
      return trust as never;
    });
    render(<DeviceTrustPanel deviceId="d" />);
    fireEvent.change(await screen.findByRole('combobox', { name: translate('auth.ownerSelect') }), { target: { value: 'c2' } });
    const add = screen.getByRole('button', { name: translate('auth.ownerAdd') });
    await waitFor(() => expect(add).toBeEnabled());
    fireEvent.click(add);
    expect(await screen.findByRole('alert')).toHaveTextContent(translate('auth.owner_origin_mismatch'));
    expect(screen.queryByRole('button', { name: translate('auth.ownerCertify') })).toBeNull();
    expect(assertOwner).toHaveBeenCalledTimes(1);
    const { appendDeviceCertificate } = await import('../lib/ownerDeviceTrust');
    expect(appendDeviceCertificate).not.toHaveBeenCalled();
  });
  it('shows unpaired relay trust and the local pairing command', async () => {
    trust.paired = false;
    trust.strict = false;
    render(<DeviceTrustPanel deviceId="d" />);
    expect(await screen.findByText('Unpaired · Relay trusted')).toBeVisible();
    expect(screen.getByText(/Run pockymoe pair on this device/)).toBeVisible();
    expect(screen.queryByText('Owner passkeys')).toBeNull();
  });
  it('shows paired/strict state, credentials, sessions and prevents removing the last credential', async () => {
    render(<DeviceTrustPanel deviceId="d" />);
    expect(await screen.findByText('Strict mode enabled')).toBeVisible();
    expect(screen.getByText('Current session')).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Remove passkey' }),
    ).toBeDisabled();
    expect(screen.getByText(/pockymoe pair --reset/)).toBeVisible();
  });
  it('revokes the current session and clears the browser key', async () => {
    render(<DeviceTrustPanel deviceId="d" />);
    fireEvent.click(
      await screen.findByRole('button', { name: 'Revoke session' }),
    );
    await waitFor(() =>
      expect(deviceTrustRequest).toHaveBeenCalledWith(
        'd',
        'sessions/s',
        'DELETE',
        undefined,
      ),
    );
    await waitFor(() => expect(clearOwnerSession).toHaveBeenCalledTimes(1));
  });
  it('removes an owner credential with its exact id', async () => {
    trust.credentials.push(second);
    render(<DeviceTrustPanel deviceId="d" />);
    fireEvent.click(
      (await screen.findAllByRole('button', { name: 'Remove passkey' }))[1]!,
    );
    await waitFor(() =>
      expect(deviceTrustRequest).toHaveBeenCalledWith(
        'd',
        'credentials/c2',
        'DELETE',
        undefined,
      ),
    );
    expect(clearOwnerSession).not.toHaveBeenCalled();
  });
  it('asserts the new credential only after the explicit add click', async () => {
    render(<DeviceTrustPanel deviceId="d" />);
    fireEvent.change(
      await screen.findByRole('combobox', {
        name: 'Choose an account passkey',
      }),
      { target: { value: 'c2' } },
    );
    const add = screen.getByRole('button', { name: 'Add another passkey' });
    await waitFor(() => expect(add).toBeEnabled());
    expect(assertOwner).not.toHaveBeenCalled();
    fireEvent.click(add);
    await waitFor(() =>
      expect(deviceTrustRequest).toHaveBeenCalledWith(
        'd',
        'credentials',
        'POST',
        {
          credential: second,
          assertion: vectors.session.registration.assertion,
        },
      ),
    );
    expect(assertOwner).toHaveBeenCalledWith([second], expect.any(Uint8Array));
    const certify = await screen.findByRole('button', {
      name: 'Verify new passkey for other browsers',
    });
    expect(assertOwner).toHaveBeenCalledTimes(1);
    fireEvent.click(certify);
    const { appendDeviceCertificate } = await import('../lib/ownerDeviceTrust');
    await waitFor(() =>
      expect(appendDeviceCertificate).toHaveBeenCalledWith(
        expect.objectContaining({
          deviceId: 'd',
          identityKey: vectors.deviceCertificate.certificate.identityKey,
          assertion: vectors.session.registration.assertion,
        }),
      ),
    );
    expect(assertOwner).toHaveBeenCalledTimes(2);
  });
});
