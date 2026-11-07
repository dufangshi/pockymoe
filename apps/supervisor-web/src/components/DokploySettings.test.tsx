import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, expect, it, vi } from 'vitest';
import { SettingsDialog } from '@pockymoe/thread-ui';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { DokploySettings } from './DokploySettings';
import * as api from '../lib/environmentApi';
import { environmentConnection, environmentsFixture, isolatedEnvironment } from '../test/environmentFixtures';
vi.mock('../lib/environmentApi', async original => ({ ...await original<typeof import('../lib/environmentApi')>(), fetchEnvironments: vi.fn(), saveEnvironmentConnection: vi.fn(), saveEnvironmentBudget: vi.fn(), testEnvironmentConnection: vi.fn(), actOnEnvironment: vi.fn() }));
beforeEach(() => {
  vi.clearAllMocks(); localStorage.clear(); setLocale('en', false);
  vi.mocked(api.fetchEnvironments).mockResolvedValue(environmentsFixture());
  vi.mocked(api.saveEnvironmentConnection).mockResolvedValue(environmentConnection);
  vi.mocked(api.testEnvironmentConnection).mockResolvedValue({ ok: true, detail: 'redacted' });
  vi.mocked(api.actOnEnvironment).mockImplementation(async (_id, action) => ({ ...isolatedEnvironment, state: action === 'destroy' ? 'destroyed' : action === 'stop' ? 'stopped' : 'starting' }));
});
async function open() {
  render(<MemoryRouter><SettingsDialog open onOpenChange={() => {}} sections={[{ id: 'environments', label: 'Isolated environments', content: <DokploySettings /> }]} /></MemoryRouter>);
  await screen.findByRole('article', { name: 'Local Dokploy' });
}
const row = () => within(screen.getByRole('article', { name: 'Local Dokploy' }));
it('edits a saved connection, never prefills its key and clears it on successful save', async () => {
  await open(); fireEvent.click(row().getByRole('button', { name: 'Edit' }));
  const editor = within(screen.getByRole('dialog', { name: 'Edit connection' }));
  expect(editor.getByLabelText(/API key/)).toHaveValue('');
  fireEvent.change(editor.getByLabelText('Name'), { target: { value: 'Renamed' } });
  fireEvent.click(editor.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(api.saveEnvironmentConnection).toHaveBeenCalledWith('dokploy-local', expect.not.objectContaining({ apiKey: expect.anything() }), null));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Edit connection' })).toBeNull());
  fireEvent.click(row().getByRole('button', { name: 'Edit' }));
  fireEvent.change(screen.getByLabelText(/API key/), { target: { value: 'synthetic-component-key' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByText('Saved');
  expect(JSON.stringify(localStorage)).not.toContain('synthetic-component-key');
  expect(document.body.textContent).not.toContain('synthetic-component-key');
});
it('protects edits when closing the nested connection form, and retains them on cancel', async () => {
  await open(); fireEvent.click(row().getByRole('button', { name: 'Edit' }));
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Keep this' } });
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  const confirm = within(await screen.findByRole('dialog', { name: 'Discard unsaved changes?' }));
  fireEvent.click(confirm.getByRole('button', { name: 'Cancel' }));
  expect(screen.getByLabelText('Name')).toHaveValue('Keep this');
  expect(api.saveEnvironmentConnection).not.toHaveBeenCalled();
});
it('accepts exact outbound domains, rejects wildcards and sends only the authorized host list', async () => {
  await open(); fireEvent.click(row().getByRole('button', { name: 'Edit' }));
  fireEvent.change(screen.getByLabelText(/Allowed outbound domains/), { target: { value: '*.example.test' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(api.saveEnvironmentConnection).not.toHaveBeenCalled();
  expect(screen.getByText('Enter exact domains without a scheme, port, path or wildcard.')).toBeVisible();
  fireEvent.change(screen.getByLabelText(/Allowed outbound domains/), { target: { value: 'github.com\nregistry.npmjs.org' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(api.saveEnvironmentConnection).toHaveBeenCalledWith('dokploy-local', expect.objectContaining({ allowedEgressHosts: ['github.com', 'registry.npmjs.org'] }), null));
});
it('tests a saved connection and keeps upstream error text out of feedback', async () => {
  vi.mocked(api.testEnvironmentConnection).mockRejectedValueOnce(new Error('sensitive upstream body'));
  await open(); fireEvent.click(row().getByRole('button', { name: 'Test connection' }));
  await waitFor(() => expect(row().getByRole('button', { name: 'Test connection' })).toBeEnabled());
  fireEvent.click(row().getByRole('button', { name: 'Why it failed' }));
  expect(await screen.findByRole('dialog', { name: 'Why it failed' })).toHaveTextContent('Connection test failed');
  expect(document.body.textContent).not.toContain('sensitive upstream body');
  fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  await screen.findByText('Connection successful');
});
it('validates budget reserves and reports the actual lifecycle after explicit destroy confirmation', async () => {
  await open(); fireEvent.click(screen.getByRole('button', { name: 'Edit budget' }));
  const budget = within(screen.getByRole('dialog', { name: 'Edit budget' }));
  fireEvent.change(budget.getByLabelText('Reserved CPU (cores)'), { target: { value: '999' } });
  expect(budget.getByRole('button', { name: 'Save' })).toBeDisabled();
  fireEvent.change(budget.getByLabelText('Reserved CPU (cores)'), { target: { value: '2' } });
  vi.mocked(api.saveEnvironmentBudget).mockResolvedValue({ ...environmentsFixture().budget, reservedCpu: 2 });
  fireEvent.click(budget.getByRole('button', { name: 'Save' })); await screen.findByText('Saved');
  const env = within(screen.getByRole('article', { name: 'Feature worktree' }));
  fireEvent.click(env.getByRole('button', { name: 'Destroy' }));
  expect(api.actOnEnvironment).not.toHaveBeenCalled();
  const confirm = within(screen.getByRole('dialog', { name: 'Destroy this environment?' }));
  expect(confirm.getByText(/Keep project files, the original working copy and environment state/)).toBeVisible();
  fireEvent.click(confirm.getByRole('button', { name: 'Destroy' }));
  await waitFor(() => expect(api.actOnEnvironment).toHaveBeenCalledWith('env-one', 'destroy', null));
  await env.findByText('Destroyed');
  act(() => setLocale('zh-CN', false));
  expect(env.getByText('已销毁')).toBeVisible();
});
