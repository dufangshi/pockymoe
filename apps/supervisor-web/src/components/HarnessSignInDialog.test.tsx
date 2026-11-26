import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { RuntimeManagement } from './RuntimeManagement';

const api = vi.hoisted(() => ({ request: vi.fn(), socket: vi.fn() }));
vi.mock('../lib/api', async (original) => ({
  ...(await original<typeof import('../lib/api')>()),
  request: api.request,
  connectShellSocket: api.socket,
  relayModeActive: () => true,
}));
// The real terminal is the shared UI's; here a stand-in reports the exit.
const terminal = vi.hoisted(() => ({ exit: () => {} }));
vi.mock('@pockymoe/thread-ui', async (original) => ({
  ...(await original<typeof import('@pockymoe/thread-ui')>()),
  SingleShellTerminal: ({ shell, onExit, shellAdapter }: { shell: { id: string }; onExit: () => void; shellAdapter: { connectSocket: (handlers: object) => unknown } }) => {
    terminal.exit = onExit;
    return <div data-testid="terminal" data-shell={shell.id}><button type="button" onClick={() => shellAdapter.connectSocket({})}>attach</button></div>;
  },
}));

const codex = (signInRequired: string | null) => ({
  id: 'codex', name: 'Codex', transport: 'adapter', baseCommand: 'codex', adapterCommand: 'codex-acp',
  signInCommand: 'codex login --device-auth', signInRequired,
  base: { installed: true, path: '/usr/bin/codex', resolvedPath: '/usr/bin/codex', manager: 'manual', canUpdate: false },
  adapter: { installed: true, path: '/usr/bin/codex-acp', resolvedPath: '/usr/bin/codex-acp', manager: 'pockymoe', canUpdate: true },
});
let harnesses: unknown[];
let jobs: Record<string, unknown>;
beforeEach(() => {
  setLocale('en', false);
  api.request.mockReset();
  api.socket.mockReset();
  harnesses = [codex('Sign-in required: OpenAI Codex is not signed in.')];
  jobs = {};
  api.request.mockImplementation(async (path: string) => {
    if (path.endsWith('/management/harnesses')) return harnesses;
    if (path.endsWith('/management/jobs')) return jobs;
    if (path.endsWith('/shells/sign-in')) return { agentId: 'codex', command: 'codex login --device-auth', shell: { id: 'login-1', status: 'running' } };
    if (path.includes('/shells/')) return {};
    return { runningVersion: '0.13.70', canUpdate: false };
  });
});
function mount() {
  return render(<MemoryRouter><RuntimeManagement view="harnesses" deviceId="a" /></MemoryRouter>);
}

it('offers Sign in for a harness with a sign-in command, highlighted when it needs one', async () => {
  mount();
  const button = await screen.findByRole('button', { name: 'Sign in to Codex' });
  expect(button).toHaveClass('harness-sign-in-needed');
  expect(button).toHaveAttribute('title', 'codex login --device-auth');
  expect(screen.getByText('Sign-in needed')).toBeVisible();
});

it('a signed-in harness keeps a plain Sign in; one without a command has none', async () => {
  harnesses = [codex(null), { ...codex(null), id: 'gemini', name: 'Gemini CLI', signInCommand: null }];
  mount();
  const button = await screen.findByRole('button', { name: 'Sign in to Codex' });
  expect(button).not.toHaveClass('harness-sign-in-needed');
  fireEvent.click(screen.getByRole('button', { name: 'Gemini CLI' }));
  await waitFor(() => expect(screen.queryByRole('button', { name: /^Sign in to/ })).toBeNull());
});

it('opens the device terminal, checks the harness after the command ends and shows the result', async () => {
  mount();
  fireEvent.click(await screen.findByRole('button', { name: 'Sign in to Codex' }));
  const dialog = await screen.findByRole('dialog', { name: 'Sign in to Codex' });
  // Only the harness id goes to the device; it runs the catalog's command.
  expect(api.request).toHaveBeenCalledWith('relay/devices/a/api/shells/sign-in', { method: 'POST', body: JSON.stringify({ agentId: 'codex' }) });
  expect(await within(dialog).findByTestId('terminal')).toHaveAttribute('data-shell', 'login-1');
  expect(dialog).toHaveTextContent('A terminal on this device runs codex login --device-auth.');
  // The terminal stream goes to the same device, outside any thread.
  fireEvent.click(within(dialog).getByRole('button', { name: 'attach' }));
  expect(api.socket).toHaveBeenCalledWith({}, { deviceId: 'a', threadId: null });

  jobs = { codex: { action: 'signIn', state: 'running' } };
  terminal.exit();
  expect(await within(dialog).findByText(/Checking the sign-in/)).toBeVisible();
  jobs = { codex: { action: 'signIn', state: 'completed', signedIn: true } };
  harnesses = [codex(null)];
  expect(await within(dialog).findByText('Signed in', {}, { timeout: 3000 })).toBeVisible();
  fireEvent.click(dialog.querySelector('[data-dialog-cancel]')!);
  // The command already ended: nothing to terminate. The row reloads.
  expect(api.request).not.toHaveBeenCalledWith(expect.stringContaining('/terminate'), expect.anything());
  await waitFor(() => expect(screen.getByRole('button', { name: 'Sign in to Codex' })).not.toHaveClass('harness-sign-in-needed'));
});

it('closing the window ends a sign-in that is still running', async () => {
  mount();
  fireEvent.click(await screen.findByRole('button', { name: 'Sign in to Codex' }));
  const dialog = await screen.findByRole('dialog', { name: 'Sign in to Codex' });
  await within(dialog).findByTestId('terminal');
  fireEvent.click(dialog.querySelector('[data-dialog-cancel]')!);
  await waitFor(() => expect(api.request).toHaveBeenCalledWith('relay/devices/a/api/shells/login-1/terminate', { method: 'POST' }));
  expect(screen.queryByRole('dialog', { name: 'Sign in to Codex' })).toBeNull();
});

it('says why a sign-in could not start', async () => {
  api.request.mockImplementation(async (path: string) => {
    if (path.endsWith('/management/harnesses')) return harnesses;
    if (path.endsWith('/shells/sign-in')) throw new Error('Executable not found: codex');
    return { runningVersion: '0.13.70', canUpdate: false };
  });
  mount();
  fireEvent.click(await screen.findByRole('button', { name: 'Sign in to Codex' }));
  const dialog = await screen.findByRole('dialog', { name: 'Sign in to Codex' });
  expect(await within(dialog).findByText('Unable to start the sign-in.')).toBeVisible();
});
