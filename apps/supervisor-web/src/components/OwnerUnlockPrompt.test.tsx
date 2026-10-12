import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import { OwnerUnlockButton, OwnerUnlockPrompt } from './OwnerUnlockPrompt';
import { prepareOwnerUnlock } from '../lib/ownerSession';
import {
  getTransportStatus,
  probeDeviceEncryption,
} from '../lib/relayTransport';
vi.mock('../lib/ownerSession', () => ({ prepareOwnerUnlock: vi.fn() }));
vi.mock('../lib/relayTransport', () => ({
  getTransportStatus: vi.fn(),
  probeDeviceEncryption: vi.fn().mockResolvedValue(undefined),
}));
vi.mock('../lib/relayTransportCrypto', () => ({
  clearTransportKeyCache: vi.fn(),
}));
vi.mock('../lib/relaySecurity', () => ({
  securityError: (e: unknown) => (e instanceof Error ? e.message : 'Failed'),
}));
const start = vi.fn();
beforeEach(() => {
  setLocale('en');
  vi.clearAllMocks();
  start.mockReset().mockResolvedValue({});
  vi.mocked(prepareOwnerUnlock).mockResolvedValue(start);
  vi.mocked(getTransportStatus).mockReturnValue({
    deviceId: 'a',
    state: 'encrypted',
    paired: true,
  });
});
function report(deviceId = 'a', revoked = false) {
  act(() => {
    window.dispatchEvent(
      new CustomEvent('remote-codex-transport', {
        detail: { deviceId, state: 'unlock-required', ...(revoked ? { unlockReason: 'owner_session_revoked' } : {}) },
      }),
    );
  });
}
describe('owner unlock prompt', () => {
  it.each(['en', 'zh-CN'] as const)('shows the distinct revocation message in %s and requires a fresh click even after dismissal', async (locale) => {
    setLocale(locale);
    render(<MemoryRouter><OwnerUnlockPrompt /></MemoryRouter>);
    report();
    expect(screen.getByText(translate('auth.ownerUnlockHelp'))).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: translate('auth.ownerLater') }));
    expect(screen.queryByRole('dialog')).toBeNull();
    report('a', true);
    expect(screen.getByText(translate('auth.owner_session_revoked'))).toBeVisible();
    expect(screen.queryByText(translate('auth.ownerUnlockHelp'))).toBeNull();
    // Subsequent requests can report "required" after the revoked key is removed.
    report('b');
    expect(screen.getByText(translate('auth.owner_session_revoked'))).toBeVisible();
    const button = await screen.findByRole('button', { name: translate('auth.ownerUnlock') });
    await waitFor(() => expect(button).toBeEnabled());
    expect(start).not.toHaveBeenCalled();
    fireEvent.click(button);
    expect(start).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  });
  it('opens on strict/session-required signals, prepares without WebAuthn and invokes the assertion only on click', async () => {
    const unlocked = vi.fn();
    window.addEventListener('remote-codex-owner-unlocked', unlocked);
    render(
      <MemoryRouter>
        <OwnerUnlockPrompt />
      </MemoryRouter>,
    );
    expect(screen.queryByRole('dialog')).toBeNull();
    report();
    report('b');
    const button = await screen.findByRole('button', {
      name: 'Unlock with passkey',
    });
    await waitFor(() => expect(button).toBeEnabled());
    expect(start).not.toHaveBeenCalled();
    fireEvent.click(button);
    expect(start).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(probeDeviceEncryption).toHaveBeenCalledWith('a');
    expect(probeDeviceEncryption).toHaveBeenCalledWith('b');
    expect(unlocked.mock.calls.map(([event]) => event.detail)).toEqual([
      { deviceIds: ['a'] }, { deviceIds: ['b'] },
    ]);
    window.removeEventListener('remote-codex-owner-unlocked', unlocked);
  });
  it('keeps a local verification mismatch blocking and never retries the device', async () => {
    start.mockRejectedValue(
      new Error('Passkey verification failed. Connection blocked.'),
    );
    render(
      <MemoryRouter>
        <OwnerUnlockPrompt />
      </MemoryRouter>,
    );
    report();
    fireEvent.click(
      await screen.findByRole('button', { name: 'Unlock with passkey' }),
    );
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Connection blocked',
    );
    expect(screen.getByRole('dialog')).toBeVisible();
    expect(probeDeviceEncryption).not.toHaveBeenCalled();
  });
  it('shows Account Security when there is no passkey and allows dismissing without a prompt', async () => {
    vi.mocked(prepareOwnerUnlock).mockRejectedValue(
      new Error('Add a passkey in Account → Security'),
    );
    render(
      <MemoryRouter>
        <OwnerUnlockPrompt />
      </MemoryRouter>,
    );
    report();
    expect(await screen.findByRole('alert')).toHaveTextContent('Add a passkey');
    expect(
      screen.getByRole('link', { name: 'Account → Security' }),
    ).toHaveAttribute('href', '/relay-account');
    fireEvent.click(screen.getByRole('button', { name: 'Later' }));
    expect(start).not.toHaveBeenCalled();
    expect(screen.queryByRole('dialog')).toBeNull();
  });
  it('does not globally prompt while pairing and supports explicit renewal clicks', async () => {
    const view = render(
      <MemoryRouter initialEntries={['/pair']}>
        <OwnerUnlockPrompt />
      </MemoryRouter>,
    );
    report();
    expect(screen.queryByRole('dialog')).toBeNull();
    view.unmount();
    const success = vi.fn();
    render(<OwnerUnlockButton renew onUnlocked={success} />);
    const button = await screen.findByRole('button', {
      name: 'Renew with passkey',
    });
    expect(start).not.toHaveBeenCalled();
    fireEvent.click(button);
    await waitFor(() => expect(success).toHaveBeenCalledTimes(1));
  });
});
