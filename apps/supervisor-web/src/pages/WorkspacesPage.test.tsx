vi.mock('../desktop/WorkspacesFrame', () => ({ useDesktopWorkspaceLayout: () => false, WorkspacesFrame: ({ children }: any) => children }));
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import type { WorkspaceDto } from '@pockymoe/shared';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { createRelayGrant, deleteWorkspace, fetchRelayPortal, fetchRuntimeConfig, fetchStorageOverview, fetchThreads, fetchWorkspaces, readSelectedRelayDeviceId, relayModeActive, setWorkspaceIsolation } from '../lib/api';
import { WorkspacesPage } from './WorkspacesPage';

vi.mock('../lib/api', () => ({
  deleteWorkspace: vi.fn(), fetchRelayPortal: vi.fn(), fetchRuntimeConfig: vi.fn(), fetchStorageOverview: vi.fn(), fetchThreads: vi.fn(),
  fetchWorkspaces: vi.fn(), setWorkspaceIsolation: vi.fn(), updateWorkspace: vi.fn(),
  updateWorkspaceFavorite: vi.fn(), relayModeActive: vi.fn(), readSelectedRelayDeviceId: vi.fn(),
  createRelayGrant: vi.fn(), updateRelayGrant: vi.fn(), revokeRelayGrant: vi.fn(),
}));
vi.mock('../components/SignedShareDetails', () => ({ SignedShareDetails: () => null }));
vi.mock('../components/DeviceEncryptionStatus', () => ({ DeviceEncryptionStatus: () => null }));
vi.mock('../components/ProductHeader', () => ({ ProductHeader: () => null }));
vi.mock('../components/onboarding/TourProvider', () => ({ TourLauncherButton: () => null }));

const workspace: WorkspaceDto = {
  id: 'workspace-1', hostId: 'host', label: 'Payroll', absPath: '/srv/payroll',
  isFavorite: false, createdAt: '2026-01-01T00:00:00Z', lastOpenedAt: null, agentIsolated: false,
};

beforeEach(() => {
  setLocale('en', false);
  vi.resetAllMocks();
  vi.mocked(fetchThreads).mockResolvedValue([]);
  vi.mocked(fetchRuntimeConfig).mockRejectedValue(new Error('offline'));
  vi.mocked(relayModeActive).mockReturnValue(false);
  vi.mocked(fetchStorageOverview).mockResolvedValue({ deviceName: 'Local desktop', scan: null });
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
    fireEvent.click(screen.getByRole('menuitem', { name: 'Isolate' }));
    await waitFor(() => expect(setWorkspaceIsolation).toHaveBeenCalledExactlyOnceWith('workspace-1', true));
    expect(await screen.findByText('Isolated')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'More actions for Payroll' }));
    expect(screen.getByRole('menuitem', { name: 'Stop isolating' })).toBeTruthy();
  });

  it('restores the row and explains a failure', async () => {
    vi.mocked(fetchWorkspaces).mockResolvedValue([workspace]);
    vi.mocked(setWorkspaceIsolation).mockRejectedValue(new Error('owner required'));
    await openMenu();
    fireEvent.click(screen.getByRole('menuitem', { name: 'Isolate' }));
    expect(await screen.findByText(/Could not change isolation/)).toBeTruthy();
    expect(screen.queryByText('Isolated')).toBeNull();
  });

  it('hides the action for a device whose runtime predates isolation', async () => {
    const { agentIsolated: _, ...older } = workspace;
    vi.mocked(fetchWorkspaces).mockResolvedValue([older]);
    await openMenu();
    expect(screen.queryByRole('menuitem', { name: 'Isolate' })).toBeNull();
  });
});

// Issue #114: a workspace is managed like a device, from one menu.
describe('the workspace menu', () => {
  it('lists the shared actions in order, at the page root, with removal marked as dangerous', async () => {
    vi.mocked(fetchWorkspaces).mockResolvedValue([workspace]);
    await openMenu();
    const menu = screen.getByRole('menu', { name: 'Actions for Payroll' });
    expect(menu.parentElement).toBe(document.body);
    const items = within(menu).getAllByRole('menuitem');
    expect(items.map(item => item.textContent)).toEqual(['Open', 'New thread', 'Rename', 'Isolate', 'View path', 'Remove…']);
    expect(items.at(-1)).toHaveAttribute('data-tone', 'danger');
    expect(within(menu).getAllByRole('separator')).toHaveLength(1);
    // The shared menu keyboard: focus starts on the first item, Esc returns it to the trigger.
    await waitFor(() => expect(items[0]).toHaveFocus());
    fireEvent.keyDown(menu, { key: 'End' });
    expect(items.at(-1)).toHaveFocus();
    fireEvent.keyDown(menu, { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    expect(screen.getByRole('button', { name: 'More actions for Payroll' })).toHaveFocus();
  });

  it('opens the same menu from a right click on the row', async () => {
    vi.mocked(fetchWorkspaces).mockResolvedValue([workspace]);
    render(<MemoryRouter><WorkspacesPage /></MemoryRouter>);
    const row = await screen.findByRole('article', { name: 'Payroll' });
    expect(fireEvent.contextMenu(row, { clientX: 40, clientY: 60 })).toBe(false);
    expect(within(screen.getByRole('menu', { name: 'Actions for Payroll' })).getAllByRole('menuitem').map(item => item.textContent))
      .toEqual(['Open', 'New thread', 'Rename', 'Isolate', 'View path', 'Remove…']);
  });

  it('asks before removing, in the same words from the title to the button', async () => {
    vi.mocked(fetchWorkspaces).mockResolvedValue([workspace]);
    vi.mocked(deleteWorkspace).mockResolvedValue(undefined as never);
    await openMenu();
    fireEvent.click(screen.getByRole('menuitem', { name: 'Remove…' }));
    const dialog = screen.getByRole('dialog', { name: 'Remove workspace?' });
    expect(dialog).toHaveTextContent('Remove Payroll and its threads from this supervisor. Files on disk are not deleted.');
    expect(dialog).not.toHaveTextContent(/delete workspace/i);
    fireEvent.click(within(dialog).getByRole('button', { name: 'Remove workspace' }));
    await waitFor(() => expect(deleteWorkspace).toHaveBeenCalledExactlyOnceWith('workspace-1', { confirmWorkspaceId: 'workspace-1', confirmLabel: 'Payroll' }));
    await waitFor(() => expect(screen.queryByRole('article', { name: 'Payroll' })).toBeNull());
  });

  it('says what "Isolated" means on a tap, not only on hover', async () => {
    vi.mocked(fetchWorkspaces).mockResolvedValue([{ ...workspace, agentIsolated: true }]);
    render(<MemoryRouter><WorkspacesPage /></MemoryRouter>);
    const mark = await screen.findByRole('button', { name: 'Isolated' });
    expect(mark).not.toHaveAttribute('title');
    // The mark is beside the row's link, not inside it: a tap does not open the workspace.
    expect(mark.closest('a')).toBeNull();
    fireEvent.click(mark);
    expect(screen.getByRole('dialog', { name: 'Isolated' })).toHaveTextContent('Agents outside this workspace cannot list, read or message its threads or read its files.');
  });
});

describe('workspace sharing from the menu', () => {
  const portal = (devices: Array<{ id: string; name: string }>) => ({ devices, sharedDevicesWithMe: [], grantsByMe: [], sharedByMe: [] }) as never;

  it('shares the workspace on the owner\'s device', async () => {
    vi.mocked(relayModeActive).mockReturnValue(true);
    vi.mocked(readSelectedRelayDeviceId).mockReturnValue('dev-1');
    vi.mocked(fetchRelayPortal).mockResolvedValue(portal([{ id: 'dev-1', name: 'Laptop' }]));
    vi.mocked(createRelayGrant).mockResolvedValue({} as never);
    vi.mocked(fetchWorkspaces).mockResolvedValue([workspace]);
    await openMenu();
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Share…' }));
    expect(await screen.findByText('No active shares for this workspace.')).toBeTruthy();
    fireEvent.change(screen.getByPlaceholderText('username or email'), { target: { value: 'carol' } });
    fireEvent.click(screen.getByRole('button', { name: 'Share workspace' }));
    await waitFor(() => expect(createRelayGrant).toHaveBeenCalledWith(expect.objectContaining({
      deviceId: 'dev-1', scope: 'workspace', workspaceId: 'workspace-1', workspaceLabel: 'Payroll', targetIdentifier: 'carol',
    })));
  });

  it('offers no sharing on a device shared with you or without the relay', async () => {
    vi.mocked(relayModeActive).mockReturnValue(true);
    vi.mocked(readSelectedRelayDeviceId).mockReturnValue('dev-2');
    vi.mocked(fetchRelayPortal).mockResolvedValue(portal([{ id: 'dev-1', name: 'Laptop' }]));
    vi.mocked(fetchWorkspaces).mockResolvedValue([workspace]);
    await openMenu();
    await waitFor(() => expect(fetchRelayPortal).toHaveBeenCalled());
    expect(screen.queryByRole('menuitem', { name: 'Share…' })).toBeNull();
  });
});
