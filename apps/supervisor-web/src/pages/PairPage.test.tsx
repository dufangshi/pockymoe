import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { PairPage } from './PairPage';
import { parsePairingFragment, takePairingOffer } from '../lib/ownerPairing';
import {
  prepareOwnerUnlock,
  fetchOwnerCredentials,
  currentOwnerSession,
} from '../lib/ownerSession';
import {
  inspectPairingDevice,
  exchange,
  type Descriptor,
} from '../lib/relayTransportCrypto';
import { fetchRelayPortal, fetchRelaySession, request } from '../lib/api';
import { getTransportStatus } from '../lib/relayTransport';
import vectors from '../lib/fixtures/ownerTrustVectors.json';
import { encode64, type OwnerCredential } from '../lib/ownerTrustCrypto';

vi.mock('../lib/api', () => ({
  enableRelayMode: vi.fn(),
  fetchRelaySession: vi.fn(),
  fetchRelayPortal: vi.fn(),
  request: vi.fn(),
}));
vi.mock('../lib/ownerSession', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../lib/ownerSession')>();
  return {
    ...actual,
    fetchOwnerCredentials: vi.fn(),
    currentOwnerSession: vi.fn(),
    assertOwner: vi.fn(),
    prepareOwnerUnlock: vi.fn(),
  };
});
vi.mock('../lib/ownerDeviceTrust', () => ({ saveDeviceCertificate: vi.fn() }));
vi.mock('../lib/relayTransportCrypto', () => ({
  inspectPairingDevice: vi.fn(),
  exchange: vi.fn(),
  finishPairingInspection: vi.fn(),
  clearTransportKeyCache: vi.fn(),
}));
vi.mock('../lib/relayTransport', () => ({
  probeDeviceEncryption: vi.fn().mockResolvedValue(undefined),
  getTransportStatus: vi.fn(),
}));
const certificate = {
  ...vectors.deviceCertificate.certificate,
  version: 1 as const,
};
const fragment = `#v=1&device=${certificate.deviceId}&pairing=${certificate.pairingId}&secret=${vectors.pairing.secret}&identity=${encode64(new Uint8Array(32))}`;
const descriptor = { identityKey: certificate.identityKey } as Descriptor;
const start = vi.fn();
beforeEach(async () => {
  vi.clearAllMocks();
  history.replaceState(null, '', '/pair' + fragment);
  vi.mocked(fetchRelaySession).mockResolvedValue({
    authenticated: true,
    user: { id: 'owner', role: 'user' },
  } as never);
  vi.mocked(fetchRelayPortal).mockResolvedValue({
    user: { id: 'owner' },
    devices: [{ id: certificate.deviceId, ownerUserId: 'owner' }],
  } as never);
  vi.mocked(inspectPairingDevice).mockResolvedValue(descriptor);
  vi.mocked(fetchOwnerCredentials).mockResolvedValue([
    vectors.credentialRecord as OwnerCredential,
  ]);
  const { assertOwner } = await import('../lib/ownerSession');
  vi.mocked(assertOwner).mockImplementation(start);
  start.mockReset().mockResolvedValue({
    credential: vectors.credentialRecord,
    assertion: certificate.assertion,
  });
  vi.mocked(exchange).mockImplementation(async () => ({
    response: Response.json({ paired: true, certificate }),
  }));
  vi.mocked(currentOwnerSession).mockResolvedValue(undefined);
  vi.mocked(prepareOwnerUnlock).mockResolvedValue(
    vi.fn().mockResolvedValue(undefined),
  );
  vi.mocked(request).mockResolvedValue({});
  vi.mocked(getTransportStatus).mockReturnValue({
    deviceId: certificate.deviceId,
    state: 'encrypted',
    paired: true,
  });
});
const mount = () =>
  render(
    <MemoryRouter initialEntries={['/pair']}>
      <PairPage />
    </MemoryRouter>,
  );
describe('pair page', () => {
  it('requires sign-in, removes the fragment and keeps secrets out of sign-in returnTo', async () => {
    vi.mocked(fetchRelaySession).mockResolvedValue({
      authenticated: false,
    } as never);
    mount();
    expect(location.hash).toBe('');
    expect(
      (await screen.findByRole('link', { name: 'Sign in' })).getAttribute(
        'href',
      ),
    ).toBe('/relay-portal');
    expect(fetchRelayPortal).not.toHaveBeenCalled();
    expect(inspectPairingDevice).not.toHaveBeenCalled();
    expect(screen.queryByText(vectors.pairing.secret)).toBeNull();
    expect(takePairingOffer().secret).toBe(vectors.pairing.secret);
  });
  it('rejects malformed/duplicate fragment fields and guides invalid links', async () => {
    for (const bad of [
      '#v=2',
      fragment + '&secret=x',
      fragment.replace(vectors.pairing.secret, 'short'),
    ])
      expect(() => parsePairingFragment(bad)).toThrow();
    history.replaceState(null, '', '/pair#v=2');
    mount();
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Invalid pairing link',
    );
    expect(fetchRelaySession).not.toHaveBeenCalled();
  });
  it('checks device ownership and fingerprint before offering a passkey click', async () => {
    mount();
    expect(
      await screen.findByRole('button', { name: 'Pair with passkey' }),
    ).toBeEnabled();
    expect(inspectPairingDevice).toHaveBeenCalledWith(
      certificate.deviceId,
      encode64(new Uint8Array(32)),
    );
    expect(start).not.toHaveBeenCalled();
  });
  it('blocks wrong-device and fingerprint mismatch states', async () => {
    vi.mocked(fetchRelayPortal).mockResolvedValue({
      user: { id: 'owner' },
      devices: [{ id: certificate.deviceId, ownerUserId: 'different-owner' }],
    } as never);
    const view = mount();
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'device identity does not match',
    );
    expect(start).not.toHaveBeenCalled();
    view.unmount();
    vi.mocked(fetchRelayPortal).mockResolvedValue({
      user: { id: 'owner' },
      devices: [{ id: certificate.deviceId, ownerUserId: 'owner' }],
    } as never);
    vi.mocked(inspectPairingDevice).mockRejectedValue(
      new Error('Fingerprint mismatch'),
    );
    mount();
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Fingerprint mismatch',
    );
    expect(start).not.toHaveBeenCalled();
  });
  it('guides accounts without a passkey to Account Security', async () => {
    vi.mocked(fetchOwnerCredentials).mockResolvedValue([]);
    mount();
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Add a passkey in Account',
    );
    expect(
      screen.getByRole('link', { name: 'Account → Security' }),
    ).toHaveAttribute('href', '/relay-account');
    expect(start).not.toHaveBeenCalled();
  });
  it('pairs only on click, sends a MAC without the secret, uploads certificate, then prompts for session unlock', async () => {
    mount();
    fireEvent.click(
      await screen.findByRole('button', { name: 'Pair with passkey' }),
    );
    expect(
      await screen.findByText(
        'Pairing saved. Unlock with your passkey to finish connecting.',
      ),
    ).toBeVisible();
    expect(start).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(exchange).mock.calls[0]![0];
    const body = await sent.clone().json();
    expect(body).toMatchObject({
      version: 1,
      pairingId: certificate.pairingId,
      mac: vectors.pairing.mac,
    });
    expect(JSON.stringify(body)).not.toContain(vectors.pairing.secret);
    expect(request).toHaveBeenCalledWith(
      `/relay/devices/${certificate.deviceId}/certificate`,
      { method: 'PUT', body: JSON.stringify(certificate) },
    );
    const unlock = await screen.findByRole('button', {
      name: 'Unlock with passkey',
    });
    await waitFor(() => expect(unlock).toBeEnabled());
    fireEvent.click(unlock);
    expect(await screen.findByRole('status')).toHaveTextContent(
      'Device paired.',
    );
  });
  it('shows expired offer and local-verification failures without uploading certificates', async () => {
    vi.mocked(exchange).mockResolvedValue({
      response: Response.json({ code: 'pairing_expired' }, { status: 410 }),
    });
    mount();
    fireEvent.click(
      await screen.findByRole('button', { name: 'Pair with passkey' }),
    );
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'pairing offer expired',
    );
    expect(request).not.toHaveBeenCalled();
  });
  it('keeps a passkey mismatch blocking before pairing or upload', async () => {
    start.mockRejectedValue(
      new Error('Passkey verification failed. Connection blocked.'),
    );
    mount();
    fireEvent.click(
      await screen.findByRole('button', { name: 'Pair with passkey' }),
    );
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Connection blocked',
    );
    expect(exchange).not.toHaveBeenCalled();
    expect(request).not.toHaveBeenCalled();
  });
  it('allows retrying certificate upload without consuming the offer twice', async () => {
    vi.mocked(request)
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValueOnce({});
    mount();
    fireEvent.click(
      await screen.findByRole('button', { name: 'Pair with passkey' }),
    );
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'device is paired, but its certificate',
    );
    fireEvent.click(
      screen.getByRole('button', { name: 'Retry saving certificate' }),
    );
    expect(
      await screen.findByText(
        'Pairing saved. Unlock with your passkey to finish connecting.',
      ),
    ).toBeVisible();
    expect(exchange).toHaveBeenCalledTimes(1);
    expect(request).toHaveBeenCalledTimes(2);
  });
});
