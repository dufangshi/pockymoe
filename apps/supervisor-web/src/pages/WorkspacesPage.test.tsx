import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import type { WorkspaceDto } from '@pockymoe/shared';
import { fetchRuntimeConfig, fetchThreads, fetchWorkspaces, setWorkspaceIsolation } from '../lib/api';
import { WorkspacesPage } from './WorkspacesPage';

vi.mock('../lib/api', () => ({
  deleteWorkspace: vi.fn(), fetchRuntimeConfig: vi.fn(), fetchThreads: vi.fn(),
  fetchWorkspaces: vi.fn(), setWorkspaceIsolation: vi.fn(), updateWorkspace: vi.fn(),
  updateWorkspaceFavorite: vi.fn(),
}));
vi.mock('../components/DeviceEncryptionStatus', () => ({ DeviceEncryptionStatus: () => null }));
vi.mock('../components/ProductHeader', () => ({ ProductHeader: () => null }));
vi.mock('../components/onboarding/TourProvider', () => ({ TourLauncherButton: () => null }));

const workspace: WorkspaceDto = {
  id: 'workspace-1', hostId: 'host', label: 'Payroll', absPath: '/srv/payroll',
  isFavorite: false, createdAt: '2026-01-01T00:00:00Z', lastOpenedAt: null, agentIsolated: false,
};

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(fetchThreads).mockResolvedValue([]);
  vi.mocked(fetchRuntimeConfig).mockRejectedValue(new Error('offline'));
});

async function openMenu() {
  render(<MemoryRouter><WorkspacesPage /></MemoryRouter>);
  await screen.findByText('Payroll');
  fireEvent.click(screen.getByRole('button', { name: 'More actions for Payroll' }));
}

describe('workspace isolation', () => {
  it('links to the workspace shell even when the workspace already has threads', async () => {
    vi.mocked(fetchWorkspaces).mockResolvedValue([workspace]);
    render(<MemoryRouter><WorkspacesPage /></MemoryRouter>);
    const link = await screen.findByRole('link', { name: /^Payroll/ });
    expect(link.getAttribute('href')).toBe('/threads?workspaceId=workspace-1');
    expect(fetchThreads).not.toHaveBeenCalled();
  });
  it('isolates a workspace from its menu and marks the row', async () => {
    vi.mocked(fetchWorkspaces).mockResolvedValue([workspace]);
    vi.mocked(setWorkspaceIsolation).mockResolvedValue({ ...workspace, agentIsolated: true });
    await openMenu();
    expect(screen.queryByText('Isolated')).toBeNull();
    fireEvent.click(screen.getByRole('menuitem', { name: 'Isolate workspace' }));
    await waitFor(() => expect(setWorkspaceIsolation).toHaveBeenCalledExactlyOnceWith('workspace-1', true));
    expect(await screen.findByText('Isolated')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'More actions for Payroll' }));
    expect(screen.getByRole('menuitem', { name: 'Stop isolating' })).toBeTruthy();
  });

  it('restores the row and explains a failure', async () => {
    vi.mocked(fetchWorkspaces).mockResolvedValue([workspace]);
    vi.mocked(setWorkspaceIsolation).mockRejectedValue(new Error('owner required'));
    await openMenu();
    fireEvent.click(screen.getByRole('menuitem', { name: 'Isolate workspace' }));
    expect(await screen.findByText(/Could not change isolation/)).toBeTruthy();
    expect(screen.queryByText('Isolated')).toBeNull();
  });

  it('hides the action for a device whose runtime predates isolation', async () => {
    const { agentIsolated: _, ...older } = workspace;
    vi.mocked(fetchWorkspaces).mockResolvedValue([older]);
    await openMenu();
    expect(screen.queryByRole('menuitem', { name: 'Isolate workspace' })).toBeNull();
  });
});
