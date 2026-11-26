import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { OutsideAgentsSettings, OutsideAgentConsent } from './OutsideAgents';
import { request } from '../lib/api';
vi.mock('../lib/api', () => ({ request: vi.fn() }));
afterEach(() => { cleanup(); vi.resetAllMocks(); history.replaceState(null, '', '/'); });
it('revokes a connected client and refreshes the list', async () => {
  vi.mocked(request).mockResolvedValue({ endpoint: 'http://127.0.0.1/mcp', clients: [{ id: 'g', clientName: 'Test client', access: 'read', workspaces: ['w'], lastUsedAt: 1 }], calls: [] });
  render(<OutsideAgentsSettings />);
  expect(await screen.findByText('Test client')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Revoke' }));
  // Irreversible: the client must sign in again, so it asks first.
  const dialog = await screen.findByRole('dialog');
  expect(dialog).toHaveTextContent('Revoke Test client?');
  expect(request).not.toHaveBeenCalledWith('api/mcp/clients/g', { method: 'DELETE' });
  fireEvent.click(within(dialog).getByRole('button', { name: 'Revoke' }));
  await waitFor(() => expect(request).toHaveBeenCalledWith('api/mcp/clients/g', { method: 'DELETE' }));
});
it('consent offers denial and never offers a broader access level', async () => {
  history.replaceState(null, '', '/?mcpConsent=ticket');
  vi.mocked(request).mockImplementation(async (path) => path === 'api/workspaces' ? [{ id: 'w', label: 'Workspace' }] : { client: { client_name: 'Test client' }, query: { redirect_uri: 'http://127.0.0.1/cb' }, requestedAccess: 'supervised' });
  render(<OutsideAgentConsent />);
  expect(await screen.findByText('Test client')).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Full access' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Read only' }));
  fireEvent.click(screen.getByLabelText('All workspaces'));
  fireEvent.click(await screen.findByLabelText('Workspace'));
  // Avoid navigating: leave the consent response pending after capturing the body.
  vi.mocked(request).mockImplementation(() => new Promise(() => {}));
  fireEvent.click(screen.getByRole('button', { name: 'Allow' }));
  await waitFor(() => expect(request).toHaveBeenCalledWith('api/mcp/consent/ticket', { method: 'POST', body: JSON.stringify({ approve: true, access: 'read', workspaces: ['w'] }) }));
  expect(screen.getByRole('button', { name: 'Deny' })).toBeEnabled();
});
it('shows tested setup for local and HTTP clients with the endpoint', async () => {
  vi.mocked(request).mockResolvedValue({ endpoint: 'http://127.0.0.1:45679/mcp', clients: [], calls: [] });
  render(<OutsideAgentsSettings />);
  fireEvent.click(screen.getByText('Setup'));
  // User scope: the default (local) scope only applies inside one project.
  expect(screen.getByText('claude mcp add --scope user pockymoe -- pockymoe mcp serve')).toBeVisible();
  expect(screen.getByText('gemini mcp add --scope user pockymoe pockymoe mcp serve')).toBeVisible();
  expect(await screen.findByText('codex mcp add pockymoe-http --url http://127.0.0.1:45679/mcp')).toBeVisible();
  expect(screen.getByText(/claude mcp login pockymoe-http/)).toBeVisible();
});
it('names workspaces and calling clients instead of showing IDs', async () => {
  vi.mocked(request).mockImplementation(async (path) => path === 'api/workspaces' ? [{ id: 'w', label: 'demo' }] : { endpoint: 'http://127.0.0.1/mcp', clients: [{ id: 'g', clientName: 'Codex', access: 'supervised', workspaces: ['w'], lastUsedAt: 1 }], calls: [{ client: 'pmcp_c', clientName: 'Codex', tool: 'pockymoe_thread_wait', target: 't', result: 'ok', time: 1 }, { client: 'local-stdio', tool: 'pockymoe_threads', target: null, result: 'error', time: 1 }] });
  render(<OutsideAgentsSettings />);
  expect(await screen.findByText(/Supervised · demo ·/)).toBeVisible();
  fireEvent.click(screen.getByText('Recent calls'));
  expect(screen.getByText(/ok · Codex/)).toBeVisible();
  expect(screen.getByText(/error · Local stdio/)).toBeVisible();
});
