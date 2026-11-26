import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
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
