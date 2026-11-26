import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, expect, it, vi } from 'vitest';
import { ThreadCreateForm } from './ThreadCreateForm';
import { environmentsFixture, isolatedEnvironment } from '../../test/environmentFixtures';
import * as environmentApi from '../../lib/environmentApi';
import { setLocale } from '@pockymoe/thread-ui/i18n';
vi.mock('../../lib/environmentApi', async original => ({ ...await original<typeof import('../../lib/environmentApi')>(), fetchEnvironments: vi.fn(), fetchThreadEnvironment: vi.fn(), actOnEnvironment: vi.fn() }));
const api = vi.hoisted(() => ({ createInWorktree: vi.fn(), create: vi.fn(), agents: vi.fn(), models: vi.fn(), install: vi.fn(), backends: vi.fn(), workspaces: vi.fn(), catalog: vi.fn(), harness: vi.fn() }));
vi.mock('../../lib/api', async original => ({
  ...(await original<typeof import('../../lib/api')>()), createThread: api.create, createThreadInWorktree: api.createInWorktree,
  fetchAgentBackendAgents: api.agents, fetchAgentBackendModelsFor: api.models,
  fetchAgentBackendModels: api.models, installOrUpdateAgentBackend: api.install,
  fetchAgentBackends: api.backends, fetchWorkspaces: api.workspaces,
  fetchAgentHarnessCatalog: api.catalog, postThreadHarnessAction: api.harness,
}));
vi.mock('../../components/HarnessSignInDialog', () => ({
  HarnessSignInDialog: ({ agentId, name, deviceId, onClose }: { agentId: string; name: string; deviceId: string | null; onClose: () => void }) => (
    <div role="dialog" aria-label={`Sign in to ${name}`} data-agent={agentId} data-device={String(deviceId)}><button type="button" onClick={onClose}>Close sign-in</button></div>
  ),
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
  const create = await screen.findByRole('button', { name: /New thread/i });
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
  const created = mount(); const create = await screen.findByRole('button', { name: /New thread/i });
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
  const create = await screen.findByRole('button', { name: /New thread/i });
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
  const create = await screen.findByRole('button', { name: /New thread/i });
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
  const create = await screen.findByRole('button', { name: /New thread/i });
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
  const create = await screen.findByRole('button', { name: /New thread/i });
  await waitFor(() => expect(create).toBeEnabled());
  expect(screen.queryByRole('button', { name: /Update ACP/i })).toBeNull();
  fireEvent.click(create);
  await waitFor(() => expect(created).toHaveBeenCalledWith({ id:'created-thread' }));
  expect(api.install).not.toHaveBeenCalled(); expect(api.harness).not.toHaveBeenCalled();
});
it('does not offer denied adapter installation in a scoped form', async () => {
  host.scoped = true; mount();
  const create = await screen.findByRole('button', { name: /New thread/i });
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
  const create = screen.getByRole('button', { name: /New thread/i });
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
  const create = screen.getByRole('button', { name: /New thread/i });
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
  await waitFor(() => expect(screen.getByRole('button', { name: /New thread/i })).toBeDisabled());
  expect(screen.getByRole('combobox', { name: 'Execution environment' })).toHaveTextContent('Local Dokploy');
  fireEvent.click(screen.getByRole('combobox', { name: 'Execution environment' }));
  expect(await screen.findByRole('option', { name: 'Local Dokploy' })).toBeDisabled();
  fireEvent.keyDown(screen.getByRole('listbox'), { key: 'Escape' });
  fireEvent.click(screen.getByRole('button', { name: 'Why it is unavailable' }));
  expect(within(await screen.findByRole('dialog', { name: 'Why it is unavailable' })).getByText('This Harness can’t run isolated; Codex, Claude and custom ACP can.')).toBeVisible();
  expect(api.create).not.toHaveBeenCalled();
});
it('allows a configured Custom ACP transport with a distinct agent id', async () => {
  api.agents.mockResolvedValue([{ ...agent(true), id: 'custom-agent', model: 'custom-agent', acpAgent: { ...agent(true).acpAgent, transport: 'custom' } }]);
  const created = mount(); await selectIsolation();
  const create = screen.getByRole('button', { name: /New thread/i });
  await waitFor(() => expect(create).toBeEnabled()); fireEvent.click(create);
  await waitFor(() => expect(created).toHaveBeenCalledTimes(1));
  expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ agentId: 'custom-agent', environmentConnectionId: 'dokploy-local' }));
});
it('does not install a Host adapter for isolation and retries only the created environment', async () => {
  vi.mocked(environmentApi.actOnEnvironment).mockRejectedValueOnce(new Error('startup failed'));
  const created = mount(); await selectIsolation();
  const agentCalls = api.agents.mock.calls.length;
  for (const mark of screen.getAllByText('Not yet verified in the isolated environment')) expect(mark).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: /New thread/i }));
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
  fireEvent.click(screen.getByRole('button', { name: /New thread/i }));
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
  fireEvent.click(screen.getByRole('button', { name: /New thread/i }));
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
  fireEvent.click(screen.getByRole('button', { name: /New thread/i }));
  await waitFor(() => expect(created).toHaveBeenCalledTimes(1));
  expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ agentId: 'codex', environmentConnectionId: 'dokploy-local' }));
  expect(api.install).not.toHaveBeenCalled(); expect(api.models).not.toHaveBeenCalled();
});

// A runtime where Codex and Claude run natively by default lists `harnessProtocol`.
const nativeEntry = (id: string, displayName: string, availability = 'ready') =>
  ({ id, model: id, displayName, isDefault: id === 'codex', selectionKind: 'agent', acpAgent: { transport: 'native', harnessProtocol: 'native', availability, baseCommand: id, statusMessage: 'Ready' } });
const acpEntry = (id: string, displayName: string, availability = 'ready', base?: string) =>
  ({ id, model: id, displayName, isDefault: false, selectionKind: 'agent', acpAgent: { transport: 'adapter', harnessProtocol: 'acp', availability, serverCommand: `${id}-acp`, installCommand: 'npm install fixture', statusMessage: 'Ready', ...(base ? { baseAgentId: base } : {}) } });
function protocolAgents(claude = 'ready', acpCodex = 'ready') {
  api.agents.mockResolvedValue([
    nativeEntry('codex', 'Codex'), nativeEntry('claude', 'Claude Code', claude),
    acpEntry('acp:codex', 'Codex', acpCodex, 'codex'), acpEntry('acp:claude', 'Claude Code', 'ready', 'claude'), acpEntry('opencode', 'OpenCode'),
  ]);
}
it('offers native Codex and Claude first and the ACP agents under one choice', async () => {
  protocolAgents();
  const created = mount();
  const group = await screen.findByRole('radiogroup', { name: 'Agent' });
  expect(within(group).getAllByRole('radio').map(radio => radio.closest('label')?.textContent)).toEqual(['Codex', 'Claude Code', 'ACP agent']);
  expect(within(group).getByRole('radio', { name: 'Codex' })).toBeChecked();
  // Only ACP to choose as the harness: no separate harness control.
  expect(screen.queryByRole('radiogroup', { name: 'Backend' })).toBeNull();
  expect(screen.queryByRole('radiogroup', { name: 'ACP agent' })).toBeNull();
  await waitFor(() => expect(api.models).toHaveBeenCalledWith('acp', expect.objectContaining({ agentId: 'codex' })));
  fireEvent.click(within(group).getByRole('radio', { name: 'ACP agent' }));
  const list = screen.getByRole('radiogroup', { name: 'ACP agent' });
  expect(within(list).getAllByRole('radio').map(radio => radio.closest('label')?.querySelector('.font-medium')?.textContent)).toEqual(['ACP · Codex', 'ACP · Claude Code', 'OpenCode']);
  expect(within(list).getByRole('radio', { name: /ACP · Codex/ })).toBeChecked();
  await waitFor(() => expect(api.models).toHaveBeenCalledWith('acp', expect.objectContaining({ agentId: 'acp:codex' })));
  fireEvent.click(within(list).getByRole('radio', { name: /OpenCode/ }));
  // Back to native and again to ACP: the ACP agent chosen last is kept.
  fireEvent.click(within(group).getByRole('radio', { name: 'Claude Code' }));
  expect(screen.queryByRole('radiogroup', { name: 'ACP agent' })).toBeNull();
  fireEvent.click(within(group).getByRole('radio', { name: 'ACP agent' }));
  expect(within(screen.getByRole('radiogroup', { name: 'ACP agent' })).getByRole('radio', { name: /OpenCode/ })).toBeChecked();
  fireEvent.click(within(screen.getByRole('radiogroup', { name: 'ACP agent' })).getByRole('radio', { name: /ACP · Claude Code/ }));
  const create = screen.getByRole('button', { name: /New thread/i });
  await waitFor(() => expect(create).toBeEnabled()); fireEvent.click(create);
  await waitFor(() => expect(created).toHaveBeenCalledTimes(1));
  expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ provider: 'acp', agentId: 'acp:claude' }));
});
it('offers only the agent list when the single-agent backends are the same agents', async () => {
  // The default device enables OpenAI Codex, Claude Agent and OpenCode beside ACP.
  api.backends.mockResolvedValue(['codex', 'claude', 'opencode', 'acp'].map(provider => ({ provider, displayName: provider, enabled: true, capabilities: { sessions: { resume: true }, turns: { start: true } }, installation: { installed: true }, status: {} })));
  protocolAgents();
  const created = mount();
  const group = await screen.findByRole('radiogroup', { name: 'Agent' });
  expect(within(group).getByRole('radio', { name: 'Codex' })).toBeChecked();
  expect(screen.queryByRole('radiogroup', { name: 'Backend' })).toBeNull();
  const create = screen.getByRole('button', { name: /New thread/i });
  await waitFor(() => expect(create).toBeEnabled()); fireEvent.click(create);
  await waitFor(() => expect(created).toHaveBeenCalledTimes(1));
  expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ provider: 'acp', agentId: 'codex' }));
});
it('creates native Codex by default and says why a native agent without its CLI cannot start', async () => {
  protocolAgents('base_missing');
  const created = mount();
  const group = await screen.findByRole('radiogroup', { name: 'Agent' });
  await waitFor(() => expect(api.models).toHaveBeenCalled());
  const modelCalls = api.models.mock.calls.length;
  fireEvent.click(within(group).getByRole('radio', { name: 'Claude Code' }));
  expect(screen.getByRole('button', { name: /New thread/i })).toBeDisabled();
  expect(api.models).toHaveBeenCalledTimes(modelCalls);
  fireEvent.click(screen.getByRole('button', { name: 'Why it is unavailable' }));
  expect(within(await screen.findByRole('dialog', { name: 'Why it is unavailable' })).getByText('Base agent missing')).toBeVisible();
  fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' });
  fireEvent.click(within(group).getByRole('radio', { name: 'Codex' }));
  const create = screen.getByRole('button', { name: /New thread/i });
  await waitFor(() => expect(create).toBeEnabled()); fireEvent.click(create);
  await waitFor(() => expect(created).toHaveBeenCalledTimes(1));
  expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ agentId: 'codex' }));
  expect(api.install).not.toHaveBeenCalled();
});
it('asks to install the adapter only for an ACP variant', async () => {
  protocolAgents('ready', 'adapter_missing');
  mount();
  fireEvent.click(within(await screen.findByRole('radiogroup', { name: 'Agent' })).getByRole('radio', { name: 'ACP agent' }));
  fireEvent.click(within(screen.getByRole('radiogroup', { name: 'ACP agent' })).getByRole('radio', { name: /ACP · Codex/ }));
  const create = screen.getByRole('button', { name: /New thread/i });
  await waitFor(() => expect(create).toBeEnabled()); fireEvent.click(create);
  expect(await screen.findByRole('dialog')).toBeVisible();
  expect(api.create).not.toHaveBeenCalled();
});
it('treats ACP Codex like Codex for isolated environments', async () => {
  protocolAgents();
  const created = mount();
  fireEvent.click(within(await screen.findByRole('radiogroup', { name: 'Agent' })).getByRole('radio', { name: 'ACP agent' }));
  await selectIsolation();
  expect(within(screen.getByRole('radiogroup', { name: 'ACP agent' })).getByRole('radio', { name: /ACP · Codex/ })).toBeChecked();
  const create = screen.getByRole('button', { name: /New thread/i });
  await waitFor(() => expect(create).toBeEnabled()); fireEvent.click(create);
  await waitFor(() => expect(created).toHaveBeenCalledTimes(1));
  expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ agentId: 'acp:codex', environmentConnectionId: 'dokploy-local' }));
});
it('creates a thread in its own worktree with a branch and base, retried with the same request id', async () => {
  installed = true;
  api.createInWorktree.mockRejectedValueOnce(new Error('network blip'))
    .mockResolvedValueOnce({ thread: { id: 'worktree-thread' }, worktree: { path: '/project-a.worktrees/keep-my-title', branch: 'feature/x' }, setup: { completed: true, output: [] }, created: true });
  const created = mount();
  const create = await screen.findByRole('button', { name: /New thread/i });
  await waitFor(() => expect(create).toBeEnabled());
  const toggle = screen.getByRole('switch', { name: 'New worktree' });
  expect(screen.queryByLabelText('Branch')).toBeNull();
  fireEvent.click(toggle);
  // Where it goes, from the title, before anything is created.
  expect(toggle.closest('.thread-create-worktree')!.textContent).toContain('/project-a.worktrees/keep-my-title');
  expect(screen.getByLabelText('Branch')).toHaveAttribute('placeholder', 'agent/keep-my-title');
  fireEvent.change(screen.getByLabelText('Branch'), { target: { value: 'feature/x' } });
  fireEvent.change(screen.getByLabelText('Base'), { target: { value: 'main~1' } });
  fireEvent.click(create);
  expect(await screen.findByRole('alert')).toHaveTextContent('network blip');
  fireEvent.click(create);
  await waitFor(() => expect(created).toHaveBeenCalledWith({ id: 'worktree-thread' }));
  expect(api.create).not.toHaveBeenCalled();
  const [first, second] = api.createInWorktree.mock.calls;
  expect(first![0]).toEqual(expect.objectContaining({ workspaceId: 'workspace-a', model: 'model-a', title: 'Keep my title' }));
  expect(first![0]).not.toHaveProperty('environmentConnectionId');
  expect(first![1]).toEqual({ branch: 'feature/x', base: 'main~1', clientRequestId: expect.any(String) });
  expect(second![1].clientRequestId).toBe(first![1].clientRequestId);
});

it('keeps a worktree thread whose setup failed, says why and opens it on request', async () => {
  installed = true;
  api.createInWorktree.mockResolvedValue({ thread: { id: 'worktree-thread' }, worktree: { path: '/p.worktrees/x', branch: 'agent/x' }, setup: { completed: false, error: 'Setup command 1 failed (exit status: 4)', output: ['npm ERR! missing'] }, created: true });
  const created = mount();
  const create = await screen.findByRole('button', { name: /New thread/i });
  await waitFor(() => expect(create).toBeEnabled());
  fireEvent.click(screen.getByRole('switch', { name: 'New worktree' }));
  fireEvent.click(create);
  expect(await screen.findByRole('alert')).toHaveTextContent('The worktree is ready but the repository setup failed');
  expect(created).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Open thread' }));
  await waitFor(() => expect(created).toHaveBeenCalledWith({ id: 'worktree-thread' }));
  expect(api.createInWorktree).toHaveBeenCalledTimes(1);
});

it('offers no worktree to a host-scoped form', async () => {
  host.scoped = true; installed = true; mount();
  await screen.findByRole('button', { name: /New thread/i });
  expect(screen.queryByRole('switch', { name: 'New worktree' })).toBeNull();
});
it('marks an agent that needs a sign-in, says how, and opens the sign-in window', async () => {
  const signInMessage = 'Sign-in required: GitHub Copilot CLI is not signed in. To use it, run `copilot login --device-code` in a terminal on this machine, then try again.';
  const signedOut = [
    agent(true),
    { ...agent(true), id: 'copilot', model: 'copilot', displayName: 'GitHub Copilot CLI', isDefault: false, acpAgent: { ...agent(true).acpAgent, transport: 'native', serverCommand: 'copilot --acp', connectionStatus: 'sign_in_required', signInCommand: 'copilot login --device-code', statusMessage: signInMessage } },
    { ...agent(true), id: 'gemini', model: 'gemini', displayName: 'Gemini CLI', isDefault: false, acpAgent: { ...agent(true).acpAgent, transport: 'native', serverCommand: 'gemini --acp', connectionStatus: 'sign_in_required', signInCommand: null, statusMessage: 'Sign-in required: Gemini CLI is not signed in.' } },
  ];
  api.agents.mockResolvedValue(signedOut);
  mount();
  const copilot = (await screen.findByText('GitHub Copilot CLI')).closest('label')!;
  expect(copilot).toHaveTextContent('Sign-in needed');
  expect(copilot).toHaveTextContent('Sign in here: this opens a terminal on the device that runs copilot login --device-code.');
  expect(within(copilot).getByRole('radio')).toBeEnabled();
  const gemini = screen.getByText('Gemini CLI').closest('label')!;
  expect(gemini).toHaveTextContent('Sign in with the agent\'s own command-line tool on this device, then try again.');
  // Only an agent with a sign-in command gets the button.
  expect(screen.getAllByRole('button', { name: /^Sign in to / })).toHaveLength(1);
  expect(screen.getByText('OpenAI Codex').closest('label')).not.toHaveTextContent('Sign-in needed');
  const agentsBefore = api.agents.mock.calls.length;
  fireEvent.click(screen.getByRole('button', { name: 'Sign in to GitHub Copilot CLI' }));
  const window = await screen.findByRole('dialog', { name: 'Sign in to GitHub Copilot CLI' });
  expect(window).toHaveAttribute('data-agent', 'copilot');
  expect(window).toHaveAttribute('data-device', 'null');
  fireEvent.click(within(window).getByRole('button', { name: 'Close sign-in' }));
  // Closing the window shows the refreshed state.
  await waitFor(() => expect(api.agents.mock.calls.length).toBeGreaterThan(agentsBefore));
  expect(screen.queryByRole('dialog', { name: /Sign in to/ })).toBeNull();
});
it('a scoped agent interface explains the sign-in but cannot open a terminal', async () => {
  host.scoped = true;
  api.agents.mockResolvedValue([agent(true), { ...agent(true), id: 'copilot', model: 'copilot', displayName: 'GitHub Copilot CLI', isDefault: false, acpAgent: { ...agent(true).acpAgent, connectionStatus: 'sign_in_required', signInCommand: 'copilot login --device-code', statusMessage: 'Sign-in required: x' } }]);
  mount();
  const copilot = (await screen.findByText('GitHub Copilot CLI')).closest('label')!;
  expect(copilot).toHaveTextContent('Run copilot login --device-code in a terminal on this device to sign in, then try again.');
  expect(screen.queryByRole('button', { name: /^Sign in to / })).toBeNull();
});
