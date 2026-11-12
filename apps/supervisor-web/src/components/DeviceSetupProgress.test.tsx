import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { currentAccount } from '../lib/accountVault';
import { request } from '../lib/api';
import { finishPendingProtection } from '../lib/finishPendingProtection';
import { ENROLLMENT_CHANGED_EVENT, listPendingEnrollments } from '../lib/pendingEnrollmentStore';
import { getTransportStatus } from '../lib/relayTransport';
import { DeviceProtectionPrompt, DeviceSetupProgress, FinishProtectionButton, usePendingProtection } from './DeviceSetupProgress';

vi.mock('../lib/accountVault', () => ({ currentAccount: vi.fn() }));
vi.mock('../lib/api', () => ({ request: vi.fn() }));
vi.mock('../lib/finishPendingProtection', () => ({ finishPendingProtection: vi.fn() }));
vi.mock('../lib/pendingEnrollmentStore', () => ({ ENROLLMENT_CHANGED_EVENT: 'pockymoe-enrollment-changed', listPendingEnrollments: vi.fn() }));
vi.mock('../lib/relayTransport', () => ({ getTransportStatus: vi.fn() }));

const account = { userId: 'u', accountKeyId: 'a', accountPublicKey: 'p' };
const pending = (deviceId: string) => ({ deviceId, userId: 'u', accountKeyId: 'a', createdAt: 1, enrollSecret: 's' });
const blocked = { deviceId: 'd', state: 'identity-changed' as const, bound: true, certificateVerified: false };
const proven = { deviceId: 'd', state: 'encrypted' as const, bound: true, certificateVerified: true, role: 'owner' as const, accountKeyId: 'a' };
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(currentAccount).mockResolvedValue(account);
  vi.mocked(listPendingEnrollments).mockResolvedValue([pending('d')]);
  vi.mocked(request).mockResolvedValue({ connected: false });
  vi.mocked(getTransportStatus).mockReturnValue(undefined as never);
});
afterEach(() => vi.useRealTimers());
const changed = () => act(() => { window.dispatchEvent(new Event(ENROLLMENT_CHANGED_EVENT)); });

it('follows a new device from waiting, through finishing, to ready', async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  render(<MemoryRouter><DeviceSetupProgress deviceId="d" /></MemoryRouter>);
  expect(await screen.findByText('Waiting for the device to connect…')).toBeVisible();
  vi.mocked(request).mockResolvedValue({ connected: true });
  await act(() => vi.advanceTimersByTimeAsync(3000));
  expect(await screen.findByText('Connected. Finishing protection…')).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Finish protection' })).toBeNull();
  // The page finished it while the account was unlocked.
  vi.mocked(listPendingEnrollments).mockResolvedValue([]);
  changed();
  expect(await screen.findByText('Connected and protected.')).toBeVisible();
  expect(screen.getByRole('link', { name: 'Open device' })).toHaveAttribute('href', '/devices/d/workspaces');
});

it('asks the reader to finish when it did not finish by itself', async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  vi.mocked(request).mockResolvedValue({ connected: true });
  vi.mocked(finishPendingProtection).mockResolvedValue(false);
  render(<MemoryRouter><DeviceSetupProgress deviceId="d" /></MemoryRouter>);
  expect(await screen.findByText('Connected. Finishing protection…')).toBeVisible();
  await act(() => vi.advanceTimersByTimeAsync(12_000));
  fireEvent.click(await screen.findByRole('button', { name: 'Finish protection' }));
  await waitFor(() => expect(finishPendingProtection).toHaveBeenCalledExactlyOnceWith('d'));
  // A failed attempt says so beside the button; it does not disappear.
  expect(await screen.findByRole('alert')).toHaveTextContent(/Could not finish protection/);
  expect(screen.getByRole('button', { name: 'Finish protection' })).toBeEnabled();
});

function Listed() { return <output>{[...usePendingProtection()].join(',') || 'none'}</output>; }
it('lists only devices that are blocked: not an unrun bind command, not a device that already shows its certificate', async () => {
  render(<Listed />);
  // Nothing heard from the device: its command may not have run, and it works as before.
  await waitFor(() => expect(listPendingEnrollments).toHaveBeenCalled());
  expect(screen.getByRole('status')).toHaveTextContent('none');
  vi.mocked(getTransportStatus).mockReturnValue(blocked as never);
  act(() => { window.dispatchEvent(new CustomEvent('remote-codex-transport', { detail: blocked })); });
  await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('d'));
  vi.mocked(getTransportStatus).mockReturnValue(proven as never);
  act(() => { window.dispatchEvent(new CustomEvent('remote-codex-transport', { detail: proven })); });
  await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('none'));
});

it('offers the unblocking step on a device page and loads again once it is done', async () => {
  vi.mocked(getTransportStatus).mockReturnValue(blocked as never);
  vi.mocked(finishPendingProtection).mockResolvedValue(true);
  const finished = vi.fn();
  render(<MemoryRouter><DeviceProtectionPrompt deviceId="d" missingCertificate onFinished={finished} /></MemoryRouter>);
  expect(await screen.findByText('This device is connected but its protection is not finished.')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Finish protection' }));
  await waitFor(() => expect(finished).toHaveBeenCalledTimes(1));
});

it('points to the device page when nothing is pending in this browser', async () => {
  vi.mocked(listPendingEnrollments).mockResolvedValue([]);
  const { rerender } = render(<MemoryRouter><DeviceProtectionPrompt deviceId="d" missingCertificate onFinished={() => {}} /></MemoryRouter>);
  expect(await screen.findByText(/has to be set up again/)).toBeVisible();
  expect(screen.getByRole('link', { name: 'Device details' })).toHaveAttribute('href', '/desktop/devices/d');
  // Another load error (offline, for example) gets no protection advice.
  rerender(<MemoryRouter><DeviceProtectionPrompt deviceId="d" missingCertificate={false} onFinished={() => {}} /></MemoryRouter>);
  expect(screen.queryByText(/has to be set up again/)).toBeNull();
});

it('keeps the compact card mark a real button with a name', async () => {
  vi.mocked(finishPendingProtection).mockResolvedValue(true);
  const finished = vi.fn();
  render(<FinishProtectionButton compact deviceId="d" onFinished={finished} />);
  fireEvent.click(screen.getByRole('button', { name: 'Finish protection' }));
  await waitFor(() => expect(finished).toHaveBeenCalled());
});
