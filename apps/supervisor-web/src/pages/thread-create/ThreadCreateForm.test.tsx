import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, expect, it, vi } from 'vitest';
import { ThreadCreateForm } from './ThreadCreateForm';
import { environmentsFixture, isolatedEnvironment } from '../../test/environmentFixtures';
import * as environmentApi from '../../lib/environmentApi';
import { setLocale } from '@pockymoe/thread-ui/i18n';
vi.mock('../../lib/environmentApi', async original => ({ ...await original<typeof import('../../lib/environmentApi')>(), fetchEnvironments: vi.fn(), fetchThreadEnvironment: vi.fn(), actOnEnvironment: vi.fn() }));
const api = vi.hoisted(() => ({ create: vi.fn(), agents: vi.fn(), models: vi.fn(), install: vi.fn(), backends: vi.fn(), workspaces: vi.fn(), catalog: vi.fn(), harness: vi.fn() }));
vi.mock('../../lib/api', async original => ({
  ...(await original<typeof import('../../lib/api')>()), createThread: api.create,
  fetchAgentBackendAgents: api.agents, fetchAgentBackendModelsFor: api.models,
  fetchAgentBackendModels: api.models, installOrUpdateAgentBackend: api.install,
  fetchAgentBackends: api.backends, fetchWorkspaces: api.workspaces,
  fetchAgentHarnessCatalog: api.catalog, postThreadHarnessAction: api.harness,
}));
vi.mock('../../components/AppShellNavContext', () => ({ useAppShellNav: () => ({ defaultBackend: 'acp' }) }));
const host = vi.hoisted(() => ({ scoped: false }));
vi.mock('../../lib/hostCapabilities', () => ({ useHostScope: () => host.scoped ? { kind:'treerAgent' } : null }));
let installed: boolean;
const agent = (ready: boolean) => ({ id: 'codex', model: 'codex', displayName: 'OpenAI Codex', isDefault: true, selectionKind: 'agent', acpAgent: { availability: ready ? 'ready' : 'adapter_missing', serverCommand: 'codex-acp', installCommand: 'npm install fixture', statusMessage: 'Ready' } });
beforeEach(() => {
  vi.resetAllMocks(); setLocale('en', false); installed = false; host.scoped = false;
  api.workspaces.mockResolvedValue([{ id: 'workspace-a', name: 'Project A', absPath: '/project-a' }]);
  api.backends.mockResolvedValue([{ provider: 'acp', displayName: 'ACP', enabled: true, capabilities: { sessions: { resume: true }, turns: { start: true } }, installation: { installed: true } }]);
  api.agents.mockImplementation(async () => [agent(installed)]);
  api.models.mockResolvedValue([{ id: 'model-a', model: 'model-a', displayName: 'Model A', isDefault: true, supportedReasoningEfforts: [], defaultReasoningEffort: null }]);
  api.install.mockImplementation(async () => { installed = true; });
  api.create.mockResolvedValue({ id: 'created-thread' });
  vi.mocked(environmentApi.fetchEnvironments).mockResolvedValue(environmentsFixture());
  vi.mocked(environmentApi.fetchThreadEnvironment).mockResolvedValue({ environment: { ...isolatedEnvironment, threadId: 'created-thread' }, supportedHarnesses: ['codex', 'claude', 'custom'], limitations: [] });
  vi.mocked(environmentApi.actOnEnvironment).mockResolvedValue({ ...isolatedEnvironment, threadId: 'created-thread', state: 'running' });
});
function mount() {
  const onCreated = vi.fn();
  render(<MemoryRouter><ThreadCreateForm initialTitle="Keep my title" onCreated={onCreated} /></MemoryRouter>);
  return onCreated;
}
it('asks before installing a missing adapter and continues the original creation after success', async () => {
  const created = mount();
  const create = await screen.findByRole('button', { name: /Create thread/i });
  await waitFor(() => expect(create).toBeEnabled());
  expect(api.install).not.toHaveBeenCalled(); expect(api.models).not.toHaveBeenCalled();
  fireEvent.click(create);
  const dialog = await screen.findByRole('dialog');
  expect(dialog).toHaveTextContent('OpenAI Codex is installed');
  expect(api.create).not.toHaveBeenCalled(); expect(api.install).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Install adapter' }));
  await waitFor(() => expect(created).toHaveBeenCalledWith({ id: 'created-thread' }));
  expect(api.install).toHaveBeenCalledTimes(1);
  expect(api.install).toHaveBeenCalledWith('acp', 'install', 'codex');
  expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ workspaceId: 'workspace-a', agentId: 'codex', model: 'model-a', title: 'Keep my title', approvalMode: 'yolo' }));
});
it('cancel leaves the adapter untouched; failed installation keeps the form and can be retried', async () => {
  const created = mount(); const create = await screen.findByRole('button', { name: /Create thread/i });
  await waitFor(() => expect(create).toBeEnabled()); fireEvent.click(create);
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(api.install).not.toHaveBeenCalled(); expect(api.create).not.toHaveBeenCalled();
  api.install.mockRejectedValueOnce(new Error('Network interrupted'));
  fireEvent.click(create); fireEvent.click(screen.getByRole('button', { name: 'Install adapter' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Network interrupted');
  expect(api.create).not.toHaveBeenCalled();
  expect(screen.getByLabelText('Title')).toHaveValue('Keep my title');
  fireEvent.click(screen.getByRole('button', { name: 'Retry installation' }));
  await waitFor(() => expect(created).toHaveBeenCalledTimes(1));
  expect(api.install).toHaveBeenCalledTimes(2);
});
it('a ready adapter creates directly without prompting or reinstalling', async () => {
  installed = true; const created = mount();
  const create = await screen.findByRole('button', { name: /Create thread/i });
  await waitFor(() => expect(create).toBeEnabled()); fireEvent.click(create);
  await waitFor(() => expect(created).toHaveBeenCalledTimes(1));
  expect(api.install).not.toHaveBeenCalled(); expect(screen.queryByRole('dialog')).toBeNull();
  expect(api.create.mock.calls[0]![0]).not.toHaveProperty('environmentConnectionId');
  expect(environmentApi.fetchThreadEnvironment).not.toHaveBeenCalled();
  expect(environmentApi.actOnEnvironment).not.toHaveBeenCalled();
});
it('does not create a thread after navigating away during installation', async () => {
  let resolve!: () => void;
  api.install.mockImplementation(() => new Promise<void>(r => { resolve = r; }));
  const onCreated = vi.fn(); const view = render(<MemoryRouter><ThreadCreateForm onCreated={onCreated} /></MemoryRouter>);
  const create = await screen.findByRole('button', { name: /Create thread/i });
  await waitFor(() => expect(create).toBeEnabled()); fireEvent.click(create);
  fireEvent.click(screen.getByRole('button', { name: 'Install adapter' }));
  view.unmount(); await act(async () => resolve());
  expect(api.create).not.toHaveBeenCalled(); expect(onCreated).not.toHaveBeenCalled();
});
it('keeps a created DSH thread when its run mode fails and retries only the mode', async () => {
  api.agents.mockResolvedValue([{ ...agent(true), id: 'deepseek', model: 'deepseek', displayName: 'DeepSeek Harness' }]);
  api.catalog.mockResolvedValue({ runModes: [{ id: 'plan', name: 'Plan', description: null, isDefault: true, broken: null }] });
  api.harness.mockRejectedValueOnce(new Error('DSH is restarting')).mockResolvedValueOnce({});
  const created = mount();
  await waitFor(() => expect(api.catalog).toHaveBeenCalled());
  const create = await screen.findByRole('button', { name: /Create thread/i });
  await waitFor(() => expect(create).toBeEnabled()); fireEvent.click(create);
  expect(await screen.findByRole('alert')).toHaveTextContent('run mode was not applied: DSH is restarting');
  expect(created).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry run mode' }));
  await waitFor(() => expect(created).toHaveBeenCalledWith({ id: 'created-thread' }));
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(api.harness).toHaveBeenCalledTimes(2);
  expect(api.harness).toHaveBeenLastCalledWith('created-thread', { kind: 'selectRunMode', id: 'plan' });
});

it('creates a scoped thread in the same page and hides runtime administration', async () => {
  host.scoped = true; installed = true;
  api.backends.mockResolvedValue([{ provider: 'acp', displayName: 'ACP', enabled: true, capabilities: { sessions: { resume: true }, turns: { start: true } }, installation: { installed: true, updateCommand:'fixture update' } }]);
  const created = mount();
  const create = await screen.findByRole('button', { name: /Create thread/i });
  await waitFor(() => expect(create).toBeEnabled());
  expect(screen.queryByRole('button', { name: /Update ACP/i })).toBeNull();
  fireEvent.click(create);
  await waitFor(() => expect(created).toHaveBeenCalledWith({ id:'created-thread' }));
  expect(api.install).not.toHaveBeenCalled(); expect(api.harness).not.toHaveBeenCalled();
});
it('does not offer denied adapter installation in a scoped form', async () => {
  host.scoped = true; mount();
  const create = await screen.findByRole('button', { name: /Create thread/i });
  await waitFor(() => expect(api.agents).toHaveBeenCalled());
  expect(create).toBeDisabled(); expect(screen.queryByRole('button',{name:'Install adapter'})).toBeNull();
  expect(api.install).not.toHaveBeenCalled();
});

async function selectIsolation() {
  const select = await screen.findByRole('combobox', { name: 'Execution environment' });
  await waitFor(() => expect(select).toBeEnabled());
  fireEvent.click(select);
  fireEvent.click(await screen.findByRole('option', { name: 'Local Dokploy' }));
  const model = screen.getByRole('textbox', { name: 'Model' });
  if (!(model as HTMLInputElement).value) fireEvent.change(model, { target: { value: 'model-a' } });
}
it('sends isolation with creation, preserves the thread after start failure, and retries only its startup', async () => {
  installed = true;
  vi.mocked(environmentApi.actOnEnvironment).mockRejectedValueOnce(new Error('raw upstream response')).mockResolvedValueOnce({ ...isolatedEnvironment, state: 'running' });
  const created = mount();
  await selectIsolation();
  const create = screen.getByRole('button', { name: /Create thread/i });
  await waitFor(() => expect(create).toBeEnabled()); fireEvent.click(create);
  const retry = await screen.findByRole('button', { name: 'Retry environment startup' });
  expect(created).not.toHaveBeenCalled();
  expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ environmentConnectionId: 'dokploy-local' }));
  expect(environmentApi.fetchThreadEnvironment).toHaveBeenCalledWith('created-thread', null);
  expect(environmentApi.actOnEnvironment).toHaveBeenCalledWith('env-one', 'start', null);
  expect(screen.getByRole('combobox', { name: 'Execution environment' })).toBeDisabled();
  expect(screen.getByLabelText('Title')).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Open thread' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Why it failed' }));
  expect(within(await screen.findByRole('dialog', { name: 'Why it failed' })).getByText(/The thread was created/)).toBeVisible();
  expect(screen.queryByText(/raw upstream response/)).toBeNull();
  fireEvent.click(retry);
  await waitFor(() => expect(created).toHaveBeenCalledExactlyOnceWith({ id: 'created-thread' }));
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(environmentApi.actOnEnvironment).toHaveBeenCalledTimes(2);
});
it('retains the thread when its binding is absent and never starts an unrelated environment', async () => {
  installed = true;
  vi.mocked(environmentApi.fetchThreadEnvironment).mockResolvedValue({ environment: null, supportedHarnesses: ['codex'], limitations: [] });
  const created = mount(); await selectIsolation();
  const create = screen.getByRole('button', { name: /Create thread/i });
  await waitFor(() => expect(create).toBeEnabled()); fireEvent.click(create);
  await screen.findByRole('button', { name: 'Retry environment startup' });
  expect(environmentApi.actOnEnvironment).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Open thread' }));
  expect(created).toHaveBeenCalledExactlyOnceWith({ id: 'created-thread' });
  expect(api.create).toHaveBeenCalledTimes(1);
});
it('disables Dokploy options for an unsupported Harness and keeps an already-selected connection from falling back to Host', async () => {
  installed = true;
  api.agents.mockResolvedValue([agent(true), { ...agent(true), id: 'unsupported', model: 'unsupported', displayName: 'Unsupported ACP', acpAgent: { ...agent(true).acpAgent, transport: 'native' } }]);
  mount(); await selectIsolation();
  fireEvent.click(screen.getByRole('radio', { name: /Unsupported ACP/ }));
  await waitFor(() => expect(screen.getByRole('button', { name: /Create thread/i })).toBeDisabled());
  expect(screen.getByRole('combobox', { name: 'Execution environment' })).toHaveTextContent('Local Dokploy');
  fireEvent.click(screen.getByRole('combobox', { name: 'Execution environment' }));
  expect(await screen.findByRole('option', { name: 'Local Dokploy' })).toBeDisabled();
  fireEvent.keyDown(screen.getByRole('listbox'), { key: 'Escape' });
  fireEvent.click(screen.getByRole('button', { name: 'Why it is unavailable' }));
  expect(within(await screen.findByRole('dialog', { name: 'Why it is unavailable' })).getByText('This Harness does not support isolated environments.')).toBeVisible();
  expect(api.create).not.toHaveBeenCalled();
});
it('allows a configured Custom ACP transport with a distinct agent id', async () => {
  api.agents.mockResolvedValue([{ ...agent(true), id: 'custom-agent', model: 'custom-agent', acpAgent: { ...agent(true).acpAgent, transport: 'custom' } }]);
  const created = mount(); await selectIsolation();
  const create = screen.getByRole('button', { name: /Create thread/i });
  await waitFor(() => expect(create).toBeEnabled()); fireEvent.click(create);
  await waitFor(() => expect(created).toHaveBeenCalledTimes(1));
  expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ agentId: 'custom-agent', environmentConnectionId: 'dokploy-local' }));
});
it('does not install a Host adapter for isolation and retries only the created environment', async () => {
  vi.mocked(environmentApi.actOnEnvironment).mockRejectedValueOnce(new Error('startup failed'));
  const created = mount(); await selectIsolation();
  const agentCalls = api.agents.mock.calls.length;
  for (const mark of screen.getAllByText('Not yet verified in the isolated environment')) expect(mark).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: /Create thread/i }));
  const retry = await screen.findByRole('button', { name: 'Retry environment startup' });
  expect(screen.queryByRole('button', { name: 'Install adapter' })).toBeNull();
  fireEvent.click(retry);
  await waitFor(() => expect(created).toHaveBeenCalledTimes(1));
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(api.install).not.toHaveBeenCalled();
  expect(api.models).not.toHaveBeenCalled();
  expect(api.catalog).not.toHaveBeenCalled();
  expect(api.agents).toHaveBeenCalledTimes(agentCalls);
});
it('aborts an outstanding Host model probe and ignores its late result after choosing isolation', async () => {
  installed = true;
  let resolve!: (models: unknown[]) => void;
  api.models.mockImplementation(() => new Promise(r => { resolve = r; }));
  const created = mount();
  await waitFor(() => expect(api.models).toHaveBeenCalledTimes(1));
  const signal = api.models.mock.calls[0]![1].signal as AbortSignal;
  await selectIsolation();
  expect(signal.aborted).toBe(true);
  fireEvent.change(screen.getByRole('textbox', { name: 'Model' }), { target: { value: 'environment-model' } });
  await act(async () => resolve([{ id: 'late-model', model: 'late-model', displayName: 'Late model', isDefault: true, supportedReasoningEfforts: [] }]));
  expect(screen.getByRole('textbox', { name: 'Model' })).toHaveValue('environment-model');
  fireEvent.click(screen.getByRole('button', { name: /Create thread/i }));
  await waitFor(() => expect(created).toHaveBeenCalledTimes(1));
  expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ model: 'environment-model', environmentConnectionId: 'dokploy-local' }));
  expect(api.models).toHaveBeenCalledTimes(1); expect(api.catalog).not.toHaveBeenCalled();
});
it('uses the supported static ACP choices after switching backend in isolation without a Host probe', async () => {
  api.backends.mockResolvedValue(['codex', 'acp'].map(provider => ({ provider, displayName: provider, enabled: true, capabilities: { sessions: { resume: true }, turns: { start: true } }, installation: { installed: false }, status: {} })));
  const created = mount();
  fireEvent.click(await screen.findByRole('radio', { name: 'codex' }));
  await selectIsolation();
  const modelsBefore = api.models.mock.calls.length, agentsBefore = api.agents.mock.calls.length;
  fireEvent.click(screen.getByRole('radio', { name: 'acp' }));
  fireEvent.click(await screen.findByRole('radio', { name: /Claude Code/ }));
  expect(screen.queryByText(/Runtime is not installed/)).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: /Create thread/i }));
  await waitFor(() => expect(created).toHaveBeenCalledTimes(1));
  expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ provider: 'acp', agentId: 'claude' }));
  expect(api.models).toHaveBeenCalledTimes(modelsBefore); expect(api.agents).toHaveBeenCalledTimes(agentsBefore);
  expect(api.install).not.toHaveBeenCalled(); expect(api.catalog).not.toHaveBeenCalled();
});
it('allows isolated creation when the supported ACP agent is missing on the Host', async () => {
  api.agents.mockResolvedValue([{ ...agent(false), acpAgent: { ...agent(false).acpAgent, availability: 'base_missing' } }]);
  const created = mount();
  await waitFor(() => expect(screen.getByRole('radio', { name: /OpenAI Codex/ })).toBeDisabled());
  await selectIsolation();
  expect(screen.getByRole('radio', { name: /OpenAI Codex/ })).toBeChecked();
  expect(screen.getByRole('radio', { name: /OpenAI Codex/ })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: /Create thread/i }));
  await waitFor(() => expect(created).toHaveBeenCalledTimes(1));
  expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ agentId: 'codex', environmentConnectionId: 'dokploy-local' }));
  expect(api.install).not.toHaveBeenCalled(); expect(api.models).not.toHaveBeenCalled();
});
