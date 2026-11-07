import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { ThreadEnvironmentControl } from './ThreadEnvironmentControl';
import { ApiError } from '../lib/api';
import * as api from '../lib/environmentApi';
import { environmentsFixture, isolatedEnvironment } from '../test/environmentFixtures';
vi.mock('../lib/environmentApi', async original => ({ ...await original<typeof import('../lib/environmentApi')>(), fetchEnvironments: vi.fn(), fetchThreadEnvironment: vi.fn(), createEnvironment: vi.fn(), bindThreadEnvironment: vi.fn() }));
const thread = { id: 'thread-one', status: 'idle' as const, provider: 'codex' as const, agentId: 'codex', providerSessionId: 'native-session', updatedAt: '2026-11-07' };
beforeEach(() => {
  vi.clearAllMocks(); setLocale('en', false);
  vi.mocked(api.fetchEnvironments).mockResolvedValue(environmentsFixture());
  vi.mocked(api.fetchThreadEnvironment).mockResolvedValue({ environment: null, supportedHarnesses: ['codex'], limitations: [] });
  vi.mocked(api.createEnvironment).mockResolvedValue({ ...isolatedEnvironment, id: 'created-env', name: 'New environment' });
  vi.mocked(api.bindThreadEnvironment).mockResolvedValue({ environment: { ...isolatedEnvironment, id: 'created-env', name: 'New environment' }, supportedHarnesses: ['codex'], limitations: [] });
});
async function open(props: Partial<Parameters<typeof ThreadEnvironmentControl>[0]> = {}) {
  render(<ThreadEnvironmentControl thread={thread} deviceId="device-a" canManage {...props} />);
  fireEvent.click(screen.getByRole('button', { name: 'Isolated environments' }));
  await screen.findByRole('button', { name: 'Create environment' });
}
it('requires explicit session release, creates only once, and binds on the captured device', async () => {
  await open(); fireEvent.click(screen.getByRole('button', { name: 'Create environment' }));
  fireEvent.change(screen.getByLabelText('Environment name (optional)'), { target: { value: 'New environment' } });
  fireEvent.click(screen.getByRole('button', { name: 'Create and use' }));
  const confirm = within(await screen.findByRole('dialog', { name: 'Release the current session and switch environments?' }));
  expect(api.createEnvironment).not.toHaveBeenCalled();
  fireEvent.click(confirm.getByRole('button', { name: 'Cancel' }));
  expect(screen.getByLabelText('Environment name (optional)')).toHaveValue('New environment');
  fireEvent.click(screen.getByRole('button', { name: 'Create and use' }));
  fireEvent.click(screen.getByRole('button', { name: 'Release session and switch' }));
  await screen.findByText('Environment applied');
  expect(api.createEnvironment).toHaveBeenCalledExactlyOnceWith({ threadId: 'thread-one', connectionId: 'dokploy-local', name: 'New environment' }, 'device-a');
  expect(api.bindThreadEnvironment).toHaveBeenCalledExactlyOnceWith('thread-one', 'created-env', 'device-a', true);
});
it('keeps a created environment selectable after binding fails, without creating a duplicate', async () => {
  vi.mocked(api.bindThreadEnvironment).mockRejectedValueOnce(new Error('upstream failure'));
  await open({ thread: { ...thread, providerSessionId: null } });
  fireEvent.click(screen.getByRole('button', { name: 'Create environment' }));
  fireEvent.click(screen.getByRole('button', { name: 'Create and use' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Apply environment' })).toBeEnabled());
  expect(screen.getByRole('combobox', { name: 'Select environment' })).toHaveTextContent('New environment');
  fireEvent.click(screen.getByRole('button', { name: 'Apply environment' }));
  await screen.findByText('Environment applied');
  expect(api.createEnvironment).toHaveBeenCalledTimes(1);
  expect(api.bindThreadEnvironment).toHaveBeenCalledTimes(2);
});
it('explains an exhausted resource budget without showing the upstream response', async () => {
  // The backend adds this code before the shared TypeScript union is regenerated.
  const code = 'environment_budget_exceeded' as ConstructorParameters<typeof ApiError>[1]['code'];
  vi.mocked(api.createEnvironment).mockRejectedValueOnce(new ApiError(409, { code, message: 'raw upstream response must remain hidden' }));
  await open({ thread: { ...thread, providerSessionId: null } });
  fireEvent.click(screen.getByRole('button', { name: 'Create environment' }));
  fireEvent.click(screen.getByRole('button', { name: 'Create and use' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Why it failed' }));
  expect(within(await screen.findByRole('dialog', { name: 'Why it failed' })).getByText('The resource budget is exhausted. Adjust it or stop another environment, then retry.')).toBeVisible();
  expect(screen.queryByText(/raw upstream response/)).toBeNull();
  expect(api.bindThreadEnvironment).not.toHaveBeenCalled();
});
it.each(['running', 'recovering'] as const)('preserves a disabled control and translated explanation for %s threads', async status => {
  await open({ thread: { ...thread, status } });
  expect(screen.getByRole('button', { name: 'Create environment' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Why it is unavailable' }));
  expect(await screen.findByText('Wait until this thread is idle before switching environments.')).toBeVisible();
  act(() => setLocale('zh-CN', false));
  expect(screen.getByRole('button', { name: '创建环境' })).toBeDisabled();
  expect(api.bindThreadEnvironment).not.toHaveBeenCalled();
});
it('does not offer another thread’s environments or destroyed environments', async () => {
  const snapshot = environmentsFixture();
  snapshot.environments.push({ ...isolatedEnvironment, id: 'other', threadId: 'another-thread', name: 'Other thread environment' }, { ...isolatedEnvironment, id: 'destroyed', state: 'destroyed', name: 'Destroyed environment' });
  vi.mocked(api.fetchEnvironments).mockResolvedValue(snapshot);
  await open({ thread: { ...thread, providerSessionId: null } });
  fireEvent.click(screen.getByRole('combobox', { name: 'Select environment' }));
  await screen.findByRole('option', { name: 'Feature worktree' });
  expect(screen.queryByRole('option', { name: 'Other thread environment' })).toBeNull();
  expect(screen.queryByRole('option', { name: 'Destroyed environment' })).toBeNull();
});
it('shows unsupported Harness and backend limitations without enabling a create request', async () => {
  vi.mocked(api.fetchThreadEnvironment).mockResolvedValue({ environment: null, supportedHarnesses: ['claude'], limitations: ['Subscription sign-in is unavailable in an isolated environment.'] });
  await open();
  expect(screen.getByRole('button', { name: 'Create environment' })).toBeDisabled();
  const marks = screen.getAllByRole('button', { name: 'Why it is unavailable' });
  fireEvent.click(marks[marks.length - 1]!);
  expect(await screen.findByText('This Harness does not support isolated environments.')).toBeVisible();
  expect(api.createEnvironment).not.toHaveBeenCalled();
});
it('accepts the custom provider family when its actual ACP agent has a distinct id', async () => {
  vi.mocked(api.fetchThreadEnvironment).mockResolvedValue({ environment: null, supportedHarnesses: ['codex', 'claude', 'custom'], limitations: [] });
  await open({ thread: { ...thread, provider: 'acp', agentId: 'my-acp-agent', providerSessionId: null } });
  expect(screen.getByRole('button', { name: 'Create environment' })).toBeEnabled();
});
it('keeps the inherited environment visible and prevents a child from switching to the host or changing its lifecycle', async () => {
  const environment = { ...isolatedEnvironment, id: 'inherited-env', threadId: 'parent-thread', name: 'Inherited environment' };
  vi.mocked(api.fetchThreadEnvironment).mockResolvedValue({ environment, supportedHarnesses: ['codex'], limitations: [] });
  await open({ thread: { ...thread, parentThreadId: 'parent-thread', providerSessionId: null } });
  expect(screen.getByRole('combobox', { name: 'Select environment' })).toHaveTextContent('Inherited environment');
  expect(screen.getByRole('combobox', { name: 'Select environment' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Create environment' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Start' })).toBeDisabled();
  fireEvent.click(screen.getAllByRole('button', { name: 'Why it is unavailable' })[0]!);
  expect(within(await screen.findByRole('dialog', { name: 'Why it is unavailable' })).getByText(/This child thread inherits/)).toBeVisible();
  expect(api.bindThreadEnvironment).not.toHaveBeenCalled();
});
