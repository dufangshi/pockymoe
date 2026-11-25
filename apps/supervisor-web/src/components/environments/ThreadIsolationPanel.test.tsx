import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { ThreadIsolationPanel } from './ThreadIsolationPanel';
import { ApiError } from '../../lib/api';
import * as api from '../../lib/environmentApi';
import { environmentsFixture, isolatedEnvironment, requestFixture } from '../../test/environmentFixtures';
vi.mock('../../lib/environmentApi', async original => ({ ...await original<typeof import('../../lib/environmentApi')>(),
  fetchEnvironments: vi.fn(), fetchEnvironmentRequests: vi.fn(), fetchThreadEnvironment: vi.fn(), createEnvironment: vi.fn(), bindThreadEnvironment: vi.fn(),
  actOnEnvironment: vi.fn(), decideEnvironmentRequest: vi.fn() }));
const thread = { id: 'thread-one', status: 'idle' as const, provider: 'codex' as const, agentId: 'codex', providerSessionId: 'native-session', updatedAt: '2026-11-07' };
const created = { ...isolatedEnvironment, id: 'created-env', name: 'New environment' };
const binding = (environment: api.EnvironmentDto | null = null, extra: Partial<api.ThreadEnvironmentDto> = {}): api.ThreadEnvironmentDto =>
  ({ environment, supportedHarnesses: ['codex'], limitations: [], ownerAuthorized: true, ...extra });
/** A snapshot in which the thread owns no environment yet. */
const bare = (): api.EnvironmentsDto => ({ ...environmentsFixture(), environments: [] });
beforeEach(() => {
  vi.clearAllMocks(); setLocale('en', false);
  vi.mocked(api.fetchEnvironmentRequests).mockResolvedValue([]);
  vi.mocked(api.fetchEnvironments).mockResolvedValue(bare());
  vi.mocked(api.fetchThreadEnvironment).mockResolvedValue(binding());
  vi.mocked(api.createEnvironment).mockResolvedValue(created);
  vi.mocked(api.bindThreadEnvironment).mockResolvedValue(binding(created));
  vi.mocked(api.actOnEnvironment).mockImplementation(async (id, action) => ({ ...created, id, state: action === 'start' ? 'running' : 'stopped' }));
});
const where = async () => within(await screen.findByRole('region', { name: 'Where it runs' }));
async function open(props: Partial<Parameters<typeof ThreadIsolationPanel>[0]> = {}) {
  render(<ThreadIsolationPanel thread={thread} deviceId="device-a" canManage {...props} />);
  const region = await where();
  await region.findByText('This device');
  return region;
}

it('marks where the thread runs, creates once after one session confirmation, binds and starts on the captured device', async () => {
  const region = await open();
  expect(region.getByRole('listitem', { current: true })).toHaveTextContent('This device');
  fireEvent.click(await region.findByRole('button', { name: /New isolated environment/ }));
  fireEvent.change(region.getByRole('textbox', { name: 'Environment name (optional)' }), { target: { value: 'New environment' } });
  fireEvent.click(region.getByRole('button', { name: 'Create and switch' }));
  const confirm = within(await screen.findByRole('dialog', { name: 'Switch where this thread runs?' }));
  expect(confirm.getByText(/starts a fresh session on New environment/)).toBeVisible();
  expect(api.createEnvironment).not.toHaveBeenCalled();
  fireEvent.click(confirm.getByRole('button', { name: 'Cancel' }));
  expect(region.getByRole('textbox', { name: 'Environment name (optional)' })).toHaveValue('New environment');
  fireEvent.click(region.getByRole('button', { name: 'Create and switch' }));
  fireEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Switch' }));
  await region.findByText('Switched');
  expect(api.createEnvironment).toHaveBeenCalledExactlyOnceWith({ threadId: 'thread-one', connectionId: 'dokploy-local', name: 'New environment' }, 'device-a');
  expect(api.bindThreadEnvironment).toHaveBeenCalledExactlyOnceWith('thread-one', 'created-env', 'device-a', true);
  // A stopped environment would hold every turn silently; using it starts it.
  expect(api.actOnEnvironment).toHaveBeenCalledExactlyOnceWith('created-env', 'start', 'device-a');
  expect(region.getByRole('listitem', { current: true })).toHaveTextContent('New environment');
  expect(region.getByRole('listitem', { current: true })).toHaveTextContent('Running');
});

it('switches without a question when no session would end, and offers the way back', async () => {
  vi.mocked(api.fetchEnvironments).mockResolvedValue(environmentsFixture());
  vi.mocked(api.bindThreadEnvironment).mockResolvedValueOnce(binding(isolatedEnvironment)).mockResolvedValueOnce(binding());
  const region = await open({ thread: { ...thread, providerSessionId: null } });
  // The thread's own environment is the other place to run; no second one can be created.
  expect(region.queryByRole('button', { name: /New isolated environment/ })).toBeNull();
  fireEvent.click(await region.findByRole('button', { name: 'Run on Feature worktree' }));
  await region.findByText('Switched');
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(api.bindThreadEnvironment).toHaveBeenLastCalledWith('thread-one', 'env-one', 'device-a', undefined);
  fireEvent.click(region.getByRole('button', { name: 'Run on this device' }));
  await waitFor(() => expect(api.bindThreadEnvironment).toHaveBeenLastCalledWith('thread-one', null, 'device-a', undefined));
  expect(region.getByRole('listitem', { current: true })).toHaveTextContent('This device');
});

it('keeps a created environment as the place to switch to after binding fails, without creating a duplicate', async () => {
  vi.mocked(api.bindThreadEnvironment).mockRejectedValueOnce(new Error('upstream failure'));
  const region = await open({ thread: { ...thread, providerSessionId: null } });
  fireEvent.click(await region.findByRole('button', { name: /New isolated environment/ }));
  fireEvent.click(region.getByRole('button', { name: 'Create and switch' }));
  const retry = await region.findByRole('button', { name: 'Run on New environment' });
  expect(region.queryByRole('button', { name: /New isolated environment/ })).toBeNull();
  fireEvent.click(retry);
  await region.findByText('Switched');
  expect(api.createEnvironment).toHaveBeenCalledTimes(1);
  expect(api.bindThreadEnvironment).toHaveBeenCalledTimes(2);
});

it('explains an exhausted budget and a second environment without showing the upstream response', async () => {
  // The backend adds this code before the shared TypeScript union is regenerated.
  const code = 'environment_budget_exceeded' as ConstructorParameters<typeof ApiError>[1]['code'];
  vi.mocked(api.createEnvironment).mockRejectedValueOnce(new ApiError(409, { code, message: 'raw upstream response must remain hidden' }))
    .mockRejectedValueOnce(new ApiError(400, { code: 'bad_request', message: 'thread already owns an environment' }));
  const region = await open({ thread: { ...thread, providerSessionId: null } });
  fireEvent.click(await region.findByRole('button', { name: /New isolated environment/ }));
  fireEvent.click(region.getByRole('button', { name: 'Create and switch' }));
  fireEvent.click(await region.findByRole('button', { name: 'Why it failed' }));
  expect(within(await screen.findByRole('dialog', { name: 'Why it failed' })).getByText('The resource budget is exhausted. Adjust it or stop another environment, then retry.')).toBeVisible();
  expect(screen.queryByText(/raw upstream response/)).toBeNull();
  fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' });
  fireEvent.click(region.getByRole('button', { name: 'Create and switch' }));
  await waitFor(() => expect(api.createEnvironment).toHaveBeenCalledTimes(2));
  fireEvent.click(await region.findByRole('button', { name: 'Why it failed' }));
  expect((await screen.findAllByText('This thread already has an environment.'))[0]).toBeVisible();
  expect(api.bindThreadEnvironment).not.toHaveBeenCalled();
});

it.each(['running', 'recovering'] as const)('says in one line why a %s thread cannot switch, in the reader’s language', async status => {
  const region = await open({ thread: { ...thread, status } });
  expect(await region.findByText('Switch when the thread is idle.')).toBeVisible();
  expect(region.getByRole('button', { name: /New isolated environment/ })).toBeDisabled();
  act(() => setLocale('zh-CN', false));
  expect(region.getByText('线程空闲后才能切换。')).toBeVisible();
  expect(region.getByRole('button', { name: /新建隔离环境/ })).toBeDisabled();
  expect(api.bindThreadEnvironment).not.toHaveBeenCalled();
});

it('says that this browser can only look when the Supervisor will not accept it as the owner', async () => {
  vi.mocked(api.fetchThreadEnvironment).mockResolvedValue(binding(null, { ownerAuthorized: false }));
  const region = await open();
  expect(await region.findByText('View only: changes need a browser signed in to your account.')).toBeVisible();
  expect(region.getByRole('button', { name: /New isolated environment/ })).toBeDisabled();
});

it('names a missing connection and where to add it instead of an unexplained mark', async () => {
  vi.mocked(api.fetchEnvironments).mockResolvedValue({ ...bare(), connections: [] });
  const region = await open();
  expect(await region.findByText('Needs a Dokploy connection first.')).toBeVisible();
  expect(region.queryByRole('button', { name: /New isolated environment/ })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Why it is unavailable' })).toBeNull();
});

it('does not offer another thread’s environments or destroyed environments', async () => {
  const snapshot = bare();
  snapshot.environments.push({ ...isolatedEnvironment, id: 'other', threadId: 'another-thread', name: 'Other thread environment' }, { ...isolatedEnvironment, id: 'destroyed', state: 'destroyed', name: 'Destroyed environment' });
  vi.mocked(api.fetchEnvironments).mockResolvedValue(snapshot);
  const region = await open({ thread: { ...thread, providerSessionId: null } });
  await region.findByRole('button', { name: /New isolated environment/ });
  expect(region.queryByText('Other thread environment')).toBeNull();
  expect(region.queryByText('Destroyed environment')).toBeNull();
});

it('shows an unsupported Harness in words and the scope in the reader’s language', async () => {
  vi.mocked(api.fetchThreadEnvironment).mockResolvedValue({ ...binding(null, { supportedHarnesses: ['claude'] }),
    limitations: ['credentials', 'disk', 'capability', 'A new backend limit'], limitationKeys: ['credentialsByRequest', 'noDiskQuota', 'capabilityHourly', 'futureKey'] });
  const region = await open();
  expect(await region.findByText('This Harness can’t run isolated; Codex, Claude and custom ACP can.')).toBeVisible();
  expect(region.getByRole('button', { name: /New isolated environment/ })).toBeDisabled();
  fireEvent.click(screen.getByText('Isolation scope and limits'));
  expect(screen.getByText('No hard disk quota')).toBeVisible();
  expect(screen.getByText('OS keychains, SSH forwarding')).toBeVisible();
  expect(screen.getByText('A new backend limit')).toBeVisible();
  expect(screen.queryByText('disk')).toBeNull();
  expect(api.createEnvironment).not.toHaveBeenCalled();
});

it('accepts the custom provider family when its actual ACP agent has a distinct id', async () => {
  vi.mocked(api.fetchThreadEnvironment).mockResolvedValue(binding(null, { supportedHarnesses: ['codex', 'claude', 'custom'] }));
  const region = await open({ thread: { ...thread, provider: 'acp', agentId: 'my-acp-agent', providerSessionId: null } });
  expect(await region.findByRole('button', { name: /New isolated environment/ })).toBeEnabled();
});

it('keeps the inherited environment visible and prevents a child from switching to the host or changing its lifecycle', async () => {
  const environment = { ...isolatedEnvironment, id: 'inherited-env', threadId: 'parent-thread', name: 'Inherited environment' };
  vi.mocked(api.fetchThreadEnvironment).mockResolvedValue(binding(environment));
  render(<ThreadIsolationPanel thread={{ ...thread, parentThreadId: 'parent-thread', providerSessionId: null }} deviceId="device-a" canManage />);
  const region = await where();
  expect(await region.findByText(/Inherited from the parent thread/)).toBeVisible();
  expect(region.getByRole('listitem', { current: true })).toHaveTextContent('Inherited environment');
  expect(region.getByRole('button', { name: 'Run on this device' })).toBeDisabled();
  expect(region.queryByRole('button', { name: /New isolated environment/ })).toBeNull();
  expect(region.getByRole('button', { name: 'Start' })).toBeDisabled();
  expect(api.bindThreadEnvironment).not.toHaveBeenCalled();
});

it('opens a request in the panel, keeps its decision reachable and returns to the banner', async () => {
  vi.mocked(api.fetchEnvironmentRequests).mockResolvedValue([requestFixture()]);
  render(<ThreadIsolationPanel thread={thread} deviceId="device-a" canManage />);
  await where();
  // The pending request is the banner; it is not listed a second time.
  fireEvent.click(await screen.findByRole('button', { name: 'Review' }));
  // In place, not a window: the heading takes focus and the decision is in the panel.
  const heading = await screen.findByRole('heading', { name: 'Agent proposal' });
  await waitFor(() => expect(heading).toHaveFocus());
  expect(screen.queryByRole('dialog')).toBeNull();
  const pane = within(screen.getByTestId('isolation-request'));
  expect(pane.getByRole('button', { name: 'Approve and migrate when idle' })).toBeEnabled();
  expect(pane.getAllByTitle('/private/native-codex').length).toBeGreaterThan(0);
  expect(api.fetchEnvironmentRequests).toHaveBeenCalledWith('device-a', 'thread-one', expect.any(AbortSignal));
  fireEvent.click(pane.getByRole('button', { name: 'Isolation requests' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Review' })).toHaveFocus());
  expect(screen.queryByRole('region', { name: 'Isolation requests' })).toBeNull();
});

it('says why an approved migration failed, blocks manual switching, and Retry first stops the failed generation', async () => {
  const failedEnvironment = { ...isolatedEnvironment, id: 'failed-env', state: 'error' as const, name: 'Agent proposal' };
  const request = { ...requestFixture(), state: 'failed' as const, approvedAt: '2026-11-07T00:00:01Z', environmentId: 'failed-env', lastError: 'Environment migration failed: environment resource budget would be exceeded' };
  vi.mocked(api.fetchEnvironmentRequests).mockResolvedValue([request]);
  vi.mocked(api.fetchEnvironments).mockResolvedValue({ ...bare(), environments: [failedEnvironment] });
  vi.mocked(api.fetchThreadEnvironment).mockResolvedValue(binding(null, { fenced: true }));
  vi.mocked(api.decideEnvironmentRequest).mockResolvedValue({ ...request, state: 'waitingIdle' });
  const region = await open({ thread: { ...thread, providerSessionId: null } });
  expect(await screen.findByText('Migration failed; Host execution fenced', { selector: 'strong' })).toBeVisible();
  expect(screen.getAllByText(/environment budget is full/)[0]).toBeVisible();
  expect(region.getByText(/An approved request decides where this runs/)).toBeVisible();
  expect(region.getByRole('button', { name: 'Run on Agent proposal' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Resolve' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Retry the approved specification' }));
  await waitFor(() => expect(api.decideEnvironmentRequest).toHaveBeenCalledWith(request, 'retry', 'device-a'));
  expect(api.actOnEnvironment).toHaveBeenCalledExactlyOnceWith('failed-env', 'stop', 'device-a');
  expect(vi.mocked(api.actOnEnvironment).mock.invocationCallOrder[0]!).toBeLessThan(vi.mocked(api.decideEnvironmentRequest).mock.invocationCallOrder[0]!);
  expect(api.bindThreadEnvironment).not.toHaveBeenCalled();
});

it('shows the current environment as one status block with labelled limits and what its state means', async () => {
  const environment = { ...isolatedEnvironment, state: 'stopped' as const };
  vi.mocked(api.fetchEnvironments).mockResolvedValue({ ...bare(), environments: [environment] });
  vi.mocked(api.fetchThreadEnvironment).mockResolvedValue(binding(environment));
  render(<ThreadIsolationPanel thread={thread} deviceId="device-a" canManage />);
  const region = await where();
  const current = await region.findByRole('listitem', { current: true });
  expect(within(current).getByText('Feature worktree')).toBeVisible();
  expect(within(current).getByText('Stopped: turns and terminals wait until it starts.')).toBeVisible();
  const limits = within(current.querySelector<HTMLElement>('dl.env-chips')!);
  expect(limits.getByText('CPU')).toBeVisible();
  expect(limits.getByText('Memory')).toBeVisible();
  expect(limits.getByText('2 GiB')).toBeVisible();
  expect(current.querySelector('code')).toHaveAttribute('title', 'example/agent:1.0');
  fireEvent.click(within(current).getByRole('button', { name: 'Start' }));
  await waitFor(() => expect(api.actOnEnvironment).toHaveBeenCalledWith('env-one', 'start', 'device-a'));
  expect(region.getByRole('button', { name: 'Run on this device' })).toBeEnabled();
});
