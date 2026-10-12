import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { AccountSessionGate } from './AccountSessionGate';
const state = vi.hoisted(() => ({ current: vi.fn(), unlock: vi.fn(), register: vi.fn() }));
vi.mock('../lib/accountVault', () => ({ currentBrowser: state.current, requireUnlock: state.unlock }));
vi.mock('../lib/accountAuth', () => ({ accountSessionState: async () => undefined }));
vi.mock('../lib/accountVaultState', () => ({ ACCOUNT_CHANGED: 'pockymoe-account-changed', registerBrowser: state.register }));
beforeEach(() => { vi.clearAllMocks(); });
it('binds a signed-in OAuth session to an existing browser certificate before opening views', async () => {
  state.current.mockResolvedValue({ browserId: 'existing' }); state.register.mockResolvedValue({ browserId: 'existing' });
  render(<AccountSessionGate><p>Device content</p></AccountSessionGate>);
  expect(await screen.findByText('Device content')).toBeVisible();
  expect(state.register).toHaveBeenCalledOnce(); expect(state.unlock).not.toHaveBeenCalled();
});
it('opens signed-in sessions without a browser key only after shared password unlock', async () => {
  state.current.mockResolvedValue(null); let finish!: () => void;
  state.unlock.mockImplementation(() => new Promise<void>(resolve => { finish = resolve; }));
  render(<AccountSessionGate><p>Device content</p></AccountSessionGate>);
  await act(async () => {}); expect(state.unlock).toHaveBeenCalledWith('connect'); expect(screen.queryByText('Device content')).toBeNull();
  state.current.mockResolvedValue({}); await act(async () => finish()); expect(await screen.findByText('Device content')).toBeVisible();
});
it('does not prompt an already certified browser and resumes after a dismissed prompt is reopened', async () => {
  state.current.mockResolvedValue(null); state.unlock.mockRejectedValue(new Error('Later'));
  render(<AccountSessionGate><p>Device content</p></AccountSessionGate>);
  expect(await screen.findByRole('alert')).toHaveTextContent('Later');
  const event = vi.fn(); window.addEventListener('remote-codex-owner-unlock-request', event);
  fireEvent.click(screen.getByRole('button')); expect(event).toHaveBeenCalledOnce();
  state.current.mockResolvedValue({}); await act(async () => window.dispatchEvent(new Event('pockymoe-account-changed')));
  expect(await screen.findByText('Device content')).toBeVisible(); window.removeEventListener('remote-codex-owner-unlock-request', event);
});
