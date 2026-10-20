import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import { AccountUnlockPrompt } from './AccountUnlockPrompt';
import { FormDialog } from './FormDialog';
import { readAfterOwnerUnlock } from '../lib/ownerUnlockRecovery';
import { ACCOUNT_UNLOCK_REQUEST, BROWSER_REMOVED } from '../lib/accountVaultState';
const state = vi.hoisted(() => ({ unlock: vi.fn(), register: vi.fn(), current: vi.fn(), session: vi.fn(), oauth: vi.fn(), logout: vi.fn() }));
vi.mock('../lib/accountVault', () => ({ currentBrowser: state.current, requireUnlock: vi.fn() }));
vi.mock('../lib/accountVaultState', () => ({ ACCOUNT_UNLOCK_REQUEST: 'pockymoe-account-unlock-request', BROWSER_REMOVED: 'pockymoe-browser-removed',
  unlockAccount: state.unlock, registerBrowser: state.register,
  AccountTrustError: class extends Error { constructor(public code: string) { super(code); } },
}));
vi.mock('../lib/accountAuth', () => ({ isOAuthAccount: state.oauth, accountSessionState: state.session }));
vi.mock('../lib/api', () => ({ relayLogout: state.logout, fetchRelayPortal: vi.fn().mockResolvedValue({ devices: [], sharedWithMe: [{ deviceId: 'dev-device-1', deviceName: 'dev' }], sharedByMe: [] }) }));
vi.mock('../lib/relayTransportCrypto', () => ({ clearTransportKeyCache: vi.fn() }));
vi.mock('../lib/relayTransport', () => ({ probeDeviceEncryption: vi.fn().mockResolvedValue(undefined), getTransportStatus: () => ({ state: 'encrypted' }) }));
beforeEach(() => {
  vi.clearAllMocks(); setLocale('en');
  state.unlock.mockResolvedValue({}); state.current.mockResolvedValue({}); state.register.mockResolvedValue({});
  state.session.mockResolvedValue({ mode: 'account', hasVault: true, recovery: false }); state.oauth.mockResolvedValue(false);
  state.logout.mockResolvedValue({});
});
function block(revoked = false) { act(() => window.dispatchEvent(new CustomEvent('remote-codex-transport', { detail: { deviceId: 'a', state: 'unlock-required', ...(revoked ? { unlockReason: 'browser_revoked' } : {}) } }))); }
function ask(reason: string, resolve = vi.fn(), reject = vi.fn()) {
  act(() => window.dispatchEvent(new CustomEvent(ACCOUNT_UNLOCK_REQUEST, { detail: { reason, resolve, reject } })));
  return { resolve, reject };
}

it('unlocks from inside a FormDialog without its focus trap blocking the password field', async () => {
  const resolve = vi.fn(), close = vi.fn();
  render(<><FormDialog title="Protect this device" onClose={close}>
    <button onClick={() => window.dispatchEvent(new CustomEvent(ACCOUNT_UNLOCK_REQUEST, {
      detail: { reason: 'bind-device', resolve, reject: vi.fn() },
    }))}>Protect</button>
  </FormDialog><AccountUnlockPrompt /></>);
  fireEvent.click(screen.getByRole('button', { name: 'Protect' }));
  const password = await screen.findByLabelText(translate('auth.password'));
  await waitFor(() => { password.focus(); expect(password).toHaveFocus(); });
  expect(screen.getByRole('dialog', { name: translate('auth.ui.confirmTitle') })).toBeVisible();
  fireEvent.change(password, { target: { value: 'local password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.unlock') }));
  await waitFor(() => expect(resolve).toHaveBeenCalledWith({}));
  expect(screen.getByRole('dialog', { name: 'Protect this device' })).toBeVisible();
  expect(close).not.toHaveBeenCalled();
});

it('keeps a busy stacked unlock open on Escape and dismisses only that dialog on Not now', async () => {
  const close = vi.fn(); let finish!: (account: object) => void;
  state.unlock.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  render(<><FormDialog title="Protect this device" onClose={close}><button>Protect</button></FormDialog><AccountUnlockPrompt /></>);
  const { reject } = ask('bind-device');
  const password = await screen.findByLabelText(translate('auth.password'));
  fireEvent.change(password, { target: { value: 'local password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.unlock') }));
  fireEvent.keyDown(password, { key: 'Escape' });
  expect(screen.getByRole('dialog', { name: translate('auth.ui.confirmTitle') })).toBeVisible();
  expect(close).not.toHaveBeenCalled(); expect(reject).not.toHaveBeenCalled();
  await act(async () => finish({}));
  const second = ask('bind-device');
  fireEvent.click(await screen.findByRole('button', { name: translate('auth.ui.later') }));
  await waitFor(() => expect(screen.getByRole('dialog', { name: 'Protect this device' })).toBeVisible());
  expect(second.reject).toHaveBeenCalledWith(expect.objectContaining({ code: 'accountUnlockCancelled' }));
  expect(close).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('shows a resumable locked notice after Not now and unlocks with a real submit in %s', async locale => {
  setLocale(locale); render(<AccountUnlockPrompt />); block();
  fireEvent.click(await screen.findByRole('button', { name: translate('auth.ui.later') }));
  expect(screen.queryByRole('dialog')).toBeNull(); expect(screen.getByRole('status')).toHaveTextContent(translate('auth.accountLocked'));
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.unlock') }));
  fireEvent.change(await screen.findByLabelText(translate('auth.password')), { target: { value: 'local password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.unlock') }));
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  expect(screen.queryByRole('status')).toBeNull(); expect(state.unlock).toHaveBeenCalledWith('local password', false);
});

it('retains removed wording after dismissal and lets a waiting read resolve via the notice', async () => {
  render(<AccountUnlockPrompt />);
  const read = vi.fn().mockRejectedValueOnce({ code: 'browser_revoked' }).mockResolvedValue({ response: Response.json({ ok: true }) });
  let waiting!: Promise<{ response: Response }>;
  await act(async () => { waiting = readAfterOwnerUnlock('a', read); });
  block(true);
  expect(await screen.findByRole('dialog', { name: translate('auth.ui.removedTitle') })).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.later') }));
  expect(screen.getByRole('status')).toHaveTextContent(translate('auth.ui.removedTitle'));
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.unlock') }));
  fireEvent.change(await screen.findByLabelText(translate('auth.password')), { target: { value: 'local password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.unlock') }));
  await act(async () => { expect((await waiting).response.ok).toBe(true); });
  expect(read).toHaveBeenCalledTimes(2); expect(screen.queryByRole('status')).toBeNull();
});

it('rejects cancelled requests and keeps the connection notice', async () => {
  render(<AccountUnlockPrompt />);
  const { reject } = ask('connect');
  fireEvent.click(await screen.findByRole('button', { name: translate('auth.ui.later') }));
  expect(reject).toHaveBeenCalledWith(expect.objectContaining({ code: 'accountUnlockCancelled' })); expect(screen.getByRole('status')).toBeVisible();
});

it('unlocks with a recovery code', async () => {
  render(<AccountUnlockPrompt />); block();
  fireEvent.click(await screen.findByRole('button', { name: translate('auth.ui.useRecoveryCode') }));
  fireEvent.change(screen.getByLabelText(translate('auth.ui.resetWithCode')), { target: { value: 'ABCD-EFGH' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.unlock') }));
  await waitFor(() => expect(state.unlock).toHaveBeenCalledWith('ABCD-EFGH', true));
});

it('sends setup and resets to the account gate instead of collecting a password', async () => {
  state.session.mockResolvedValue({ mode: 'legacy', hasVault: false, recovery: false });
  render(<AccountUnlockPrompt />); block();
  expect(await screen.findByRole('button', { name: translate('auth.ui.setupContinue') })).toBeVisible();
  expect(screen.queryByLabelText(translate('auth.password'))).toBeNull();
});

it('offers sign out from the prompt', async () => {
  const assign = vi.fn(); Object.defineProperty(window, 'location', { configurable: true, value: { ...window.location, assign } });
  render(<AccountUnlockPrompt />); block();
  fireEvent.click(await screen.findByRole('button', { name: translate('auth.ui.signOut') }));
  await waitFor(() => expect(assign).toHaveBeenCalledWith('/relay-portal'));
  expect(state.logout).toHaveBeenCalledOnce();
});

it('explains a browser that found itself in the account-signed revocation list', async () => {
  render(<AccountUnlockPrompt />);
  act(() => { window.dispatchEvent(new Event(BROWSER_REMOVED)); });
  ask('connect');
  expect(await screen.findByRole('dialog', { name: translate('auth.ui.removedTitle') })).toBeVisible();
});

it('says which device refused which request, and copies the details', async () => {
  render(<AccountUnlockPrompt />);
  act(() => window.dispatchEvent(new CustomEvent('remote-codex-transport', { detail: {
    deviceId: 'dev-device-1', state: 'unlock-required',
    unlockCause: { kind: 'device-refused', deviceId: 'dev-device-1', method: 'GET', path: '/api/threads/t1/subagents', code: 'browser_registration_required', at: Date.now() },
  } })));
  expect(await screen.findByText(translate('auth.unlockCause.deviceRefused', { device: 'dev' }))).toBeVisible();
  expect(screen.getByText(/GET \/api\/threads\/t1\/subagents · browser_registration_required/)).toBeVisible();
  expect(screen.getByRole('button', { name: translate('auth.unlockCause.copy') })).toBeVisible();
});

it('names the action an explicit unlock is for', async () => {
  render(<AccountUnlockPrompt />);
  ask('bind-device');
  expect(await screen.findByText(translate('auth.unlockCause.action', { action: translate('auth.unlockAction.bindDevice') }))).toBeVisible();
});
