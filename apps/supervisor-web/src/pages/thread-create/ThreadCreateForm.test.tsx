import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, expect, it, vi } from 'vitest';
import { ThreadCreateForm } from './ThreadCreateForm';
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
  vi.clearAllMocks(); installed = false; host.scoped = false;
  api.workspaces.mockResolvedValue([{ id: 'workspace-a', name: 'Project A', absPath: '/project-a' }]);
  api.backends.mockResolvedValue([{ provider: 'acp', displayName: 'ACP', enabled: true, capabilities: { sessions: { resume: true }, turns: { start: true } }, installation: { installed: true } }]);
  api.agents.mockImplementation(async () => [agent(installed)]);
  api.models.mockResolvedValue([{ id: 'model-a', model: 'model-a', displayName: 'Model A', isDefault: true, supportedReasoningEfforts: [], defaultReasoningEffort: null }]);
  api.install.mockImplementation(async () => { installed = true; });
  api.create.mockResolvedValue({ id: 'created-thread' });
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
