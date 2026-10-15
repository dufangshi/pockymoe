import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import { beforeEach, expect, it, vi } from 'vitest';
import { LocalPasscodeSettings } from './LocalPasscodeSettings';
import { LoginPage } from '../pages/LoginPage';

const api = vi.hoisted(() => ({ session: vi.fn(), relay: vi.fn(), set: vi.fn(), remove: vi.fn() }));
vi.mock('../lib/api', () => ({
  ApiError: class extends Error { constructor(public payload: { message: string }) { super(payload.message); } },
  fetchAuthSession: api.session, relayModeActive: api.relay, setLocalPasscode: api.set, removeLocalPasscode: api.remove,
}));
const session = (passcode: boolean, mode = 'local') => ({ authenticated: true, username: null, expiresAt: null, mode, authRequired: passcode, passcode });
beforeEach(() => {
  vi.resetAllMocks(); setLocale('en'); api.relay.mockReturnValue(false);
});

it('is not shown for a relay device or a server-mode Supervisor', async () => {
  api.relay.mockReturnValue(true);
  const { container, unmount } = render(<LocalPasscodeSettings />);
  expect(container).toBeEmptyDOMElement(); expect(api.session).not.toHaveBeenCalled();
  unmount(); api.relay.mockReturnValue(false); api.session.mockResolvedValue(session(false, 'server'));
  const server = render(<LocalPasscodeSettings />);
  await waitFor(() => expect(api.session).toHaveBeenCalled());
  expect(server.container).toBeEmptyDOMElement();
});

it('turns a passcode on, changes it and turns it off', async () => {
  api.session.mockResolvedValue(session(false));
  api.set.mockResolvedValue({ token: 't', session: session(true) });
  api.remove.mockResolvedValue(session(false));
  vi.spyOn(window, 'confirm').mockReturnValue(true);
  render(<LocalPasscodeSettings />);
  expect(await screen.findByText(translate('settings.passcodeOffDescription'))).toBeVisible();
  const input = screen.getByLabelText(translate('settings.passcodeLabel'));
  fireEvent.change(input, { target: { value: 'abc' } });
  fireEvent.click(screen.getByRole('button', { name: translate('settings.passcodeEnable') }));
  expect(screen.getByRole('alert')).toHaveTextContent(translate('settings.passcodeTooShort'));
  expect(api.set).not.toHaveBeenCalled();
  fireEvent.change(input, { target: { value: 'open sesame' } });
  fireEvent.click(screen.getByRole('button', { name: translate('settings.passcodeEnable') }));
  expect(await screen.findByText(translate('settings.passcodeOnDescription'))).toBeVisible();
  expect(api.set).toHaveBeenCalledWith('open sesame');
  expect(screen.getByText(translate('settings.passcodeForgot'))).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: translate('settings.passcodeDisable') }));
  expect(await screen.findByText(translate('settings.passcodeOffDescription'))).toBeVisible();
  expect(api.remove).toHaveBeenCalledOnce();
});

it.each(['en', 'zh-CN'] as const)('asks only for the passcode when the Supervisor uses one (%s)', async locale => {
  setLocale(locale);
  const onLogin = vi.fn().mockResolvedValue(undefined);
  render(<LoginPage onLogin={onLogin} passcode />);
  expect(screen.getByRole('heading', { name: translate('auth.passcodeTitle') })).toBeVisible();
  expect(screen.queryByLabelText(translate('auth.username'))).toBeNull();
  fireEvent.change(screen.getByLabelText(translate('auth.passcodeLabel')), { target: { value: 'open sesame' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.signIn') }));
  await waitFor(() => expect(onLogin).toHaveBeenCalledWith({ username: '', password: 'open sesame' }));
});
