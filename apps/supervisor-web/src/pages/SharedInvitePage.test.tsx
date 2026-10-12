import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
const state = vi.hoisted(() => ({
  session: vi.fn(),
  pin: vi.fn(),
  fingerprint: vi.fn(),
  clear: vi.fn(),
}));
vi.mock('../lib/api', () => ({
  enableRelayMode: vi.fn(),
  fetchRelaySession: state.session,
  fetchRelayPortal: async () => ({ devices: [{ id: 'device', name: 'Test device' }] }),
}));
vi.mock('../lib/sharedDeviceTrust', () => ({ pinInviteOwner: state.pin, checkSharedInviteDevice: state.fingerprint }));
vi.mock('../lib/relayTransportCrypto', () => ({
  clearTransportKeyCache: state.clear,
}));
import { SharedInvitePage } from './SharedInvitePage';
const identity = 'g'.repeat(43);
function Destination() {
  const location = useLocation();
  return <p data-testid="destination">{location.pathname}</p>;
}
function mount() {
  return render(
    <MemoryRouter initialEntries={['/shared']}>
      <Routes>
        <Route path="/shared" element={<SharedInvitePage />} />
        <Route path="*" element={<Destination />} />
      </Routes>
    </MemoryRouter>,
  );
}
beforeEach(() => {
  state.pin.mockReset().mockResolvedValue(undefined);
  state.fingerprint.mockReset().mockResolvedValue(identity);
  state.clear.mockReset();
  state.session.mockReset().mockResolvedValue({ authenticated: true });
  sessionStorage.clear();
  window.history.replaceState(
    null,
    '',
    '/shared#v=2&device=device&owner=' + identity + '&thread=thread',
  );
});
it('pins the invite before checking the signed descriptor, then navigates to the encoded thread', async () => {
  mount();
  expect(state.pin).not.toHaveBeenCalled();
  expect(await screen.findByText('Device: Test device (device)')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Confirm owner account key' }));
  expect(await screen.findByTestId('destination')).toHaveTextContent(
    '/devices/device/threads/thread',
  );
  expect(state.pin).toHaveBeenCalledWith('device', identity);
  expect(state.fingerprint).toHaveBeenCalledWith('device', 'thread');
  expect(state.pin.mock.invocationCallOrder[0]).toBeLessThan(
    state.fingerprint.mock.invocationCallOrder[0]!,
  );
  expect(sessionStorage.getItem('pockymoe-pending-invite')).toBeNull();
});
it('keeps the fragment across sign-in without saving a pin without sending them in the login URL', async () => {
  state.session.mockResolvedValue({ authenticated: false });
  const rendered = mount();
  expect(state.pin).not.toHaveBeenCalled();
  const signin = await screen.findByRole('link', { name: 'Sign in' });
  expect(signin).toHaveAttribute('href', '/relay-portal');
  expect(state.pin).not.toHaveBeenCalled();
  expect(state.fingerprint).not.toHaveBeenCalled();
  rendered.unmount();
  window.history.replaceState(null, '', '/shared');
  state.session.mockResolvedValue({ authenticated: true });
  state.pin.mockClear();
  mount();
  expect(state.pin).not.toHaveBeenCalled();
  expect(await screen.findByText('Device: Test device (device)')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Confirm owner account key' }));
  expect(await screen.findByTestId('destination')).toHaveTextContent(
    '/devices/device/threads/thread',
  );
});
it('shows a hard mismatch error instead of navigating or attempting to unlock', async () => {
  state.fingerprint.mockRejectedValue(new Error('Device identity mismatch'));
  mount();
  fireEvent.click(await screen.findByRole('button', { name: 'Confirm owner account key' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Device identity mismatch',
  );
  expect(screen.queryByTestId('destination')).not.toBeInTheDocument();
});
it('rejects malformed fragments before a relay request', async () => {
  window.history.replaceState(null, '', '/shared#v=1&device=device');
  mount();
  expect(await screen.findByRole('alert')).toHaveTextContent(/invalid/);
  expect(state.session).not.toHaveBeenCalled();
  expect(state.pin).not.toHaveBeenCalled();
});
