import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { RelayPortalSummaryDto, RuntimeConfigDto, ThreadDetailDto } from '@pockymoe/shared';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { RecentThreadMenu } from './RecentThreadMenu';
import { HostCapabilitiesProvider } from '../lib/hostCapabilities';
import {
  createRelayGrant, createRelayShare, fetchRelayAccess, fetchRelayPortal, fetchRuntimeConfig,
  relayModeActive, request, revokeRelayShare, updateRelayShare,
} from '../lib/api';

vi.mock('../lib/api', async original => ({
  ...(await original<typeof import('../lib/api')>()),
  fetchRuntimeConfig: vi.fn(), request: vi.fn(), fetchRelayAccess: vi.fn(), relayModeActive: vi.fn(),
  fetchRelayPortal: vi.fn(), createRelayShare: vi.fn(), createRelayGrant: vi.fn(),
  updateRelayShare: vi.fn(), revokeRelayShare: vi.fn(),
}));
// The trust details verify signatures; the share list only needs a row.
vi.mock('./SignedShareDetails', () => ({ SignedShareDetails: () => null }));

const relayThread = { key: 'dev-1:t-1', title: 'Fix login', href: '/relay/devices/dev-1/threads/t-1', favorite: false, subtitle: 'App', status: 'idle' };
const detail = { thread: { id: 't-1', title: 'Fix login', workspaceId: 'ws-1' }, workspace: { id: 'ws-1', label: 'App' } } as ThreadDetailDto;
const share = {
  id: 'share-1', deviceId: 'dev-1', threadId: 't-1', threadTitle: 'Fix login', workspaceId: null, workspaceLabel: null,
  targetUsername: 'bob', label: null, threadAccess: 'read', workspaceAccess: 'none', createdAt: '2026-10-25T00:00:00Z',
};
const portal = (extra: Partial<RelayPortalSummaryDto> = {}) => ({ sharedByMe: [], grantsByMe: [], ...extra }) as unknown as RelayPortalSummaryDto;

beforeEach(() => {
  setLocale('en', false);
  vi.mocked(fetchRuntimeConfig).mockResolvedValue({} as RuntimeConfigDto);
  vi.mocked(request).mockResolvedValue(detail);
  vi.mocked(relayModeActive).mockReturnValue(true);
  vi.mocked(fetchRelayAccess).mockResolvedValue({ kind: 'owner' } as never);
  vi.mocked(fetchRelayPortal).mockResolvedValue(portal());
  vi.mocked(createRelayShare).mockResolvedValue({} as never);
  vi.mocked(createRelayGrant).mockResolvedValue({} as never);
  vi.mocked(updateRelayShare).mockResolvedValue({} as never);
  vi.mocked(revokeRelayShare).mockResolvedValue({} as never);
});
afterEach(() => { cleanup(); vi.clearAllMocks(); });

// The provider's scope is the device the page is on, as in the app.
function mount(thread = relayThread, scope = 'dev-1') {
  render(<HostCapabilitiesProvider scope={scope}><RecentThreadMenu thread={thread} currentKey="" onFavorite={vi.fn()} onRenamed={vi.fn()} onRemoved={vi.fn()} onNavigate={vi.fn()} /></HostCapabilitiesProvider>);
}
async function openMenu(title = relayThread.title) {
  fireEvent.click(await screen.findByRole('button', { name: `Actions for ${title}` }));
  // The owner's own items arrive with the thread's details.
  await screen.findByRole('button', { name: 'Rename thread' });
  return screen.getByRole('dialog', { name: `Thread actions: ${title}` });
}

it('offers Share thread to the owner of a relay device, before rename and delete', async () => {
  mount();
  const menu = await openMenu();
  const labels = within(menu).getAllByRole('button').map(item => item.textContent);
  expect(labels.indexOf('Share thread')).toBeGreaterThan(labels.indexOf('Copy Pockymoe session ID'));
  expect(labels.indexOf('Share thread')).toBeLessThan(labels.indexOf('Rename thread'));
  expect(within(menu).getByRole('button', { name: 'Share thread' }).querySelector('svg')?.getAttribute('data-icon')).toBe('share');
  expect(fetchRelayAccess).toHaveBeenCalledWith({ deviceId: 'dev-1', threadId: 't-1' });
});

it('leaves Share thread out when this account cannot share the thread', async () => {
  // A recipient of the thread: no share, rename or delete.
  vi.mocked(fetchRelayAccess).mockResolvedValue({ kind: 'shared', threadAccess: 'control', workspaceAccess: 'none' } as never);
  mount();
  fireEvent.click(await screen.findByRole('button', { name: 'Actions for Fix login' }));
  await waitFor(() => expect(request).toHaveBeenCalled());
  await screen.findByRole('button', { name: 'Copy harness session ID' });
  expect(screen.queryByRole('button', { name: 'Share thread' })).toBeNull();
  cleanup();
  // A direct Supervisor has no relay to share through.
  vi.mocked(relayModeActive).mockReturnValue(false);
  mount({ ...relayThread, key: 'local:t-1', href: '/threads/t-1' }, 'local');
  await openMenu();
  expect(screen.queryByRole('button', { name: 'Share thread' })).toBeNull();
});

it('leaves Share thread out of a host-scoped workbench', async () => {
  vi.mocked(fetchRuntimeConfig).mockResolvedValue({ hostScope: { kind: 'treerAgent', hubId: 'hub', boundThreadId: 'root' } } as RuntimeConfigDto);
  mount();
  await openMenu();
  expect(screen.queryByRole('button', { name: 'Share thread' })).toBeNull();
});

it('opens the share window for that thread and shares it with someone', async () => {
  vi.mocked(fetchRelayPortal).mockResolvedValue(portal({ sharedByMe: [share, { ...share, id: 'other', threadId: 't-2' }] } as never));
  mount();
  await openMenu();
  fireEvent.click(screen.getByRole('button', { name: 'Share thread' }));
  const window = await screen.findByRole('dialog', { name: 'Thread actions' });
  expect(within(window).getByText('Sharing permissions')).toBeTruthy();
  // Only this thread's own shares are listed.
  await within(window).findByText('bob');
  expect(within(window).getAllByRole('button', { name: 'Revoke' })).toHaveLength(1);
  // The menu closed; the window offers this thread, its workspace and the device.
  expect(screen.queryByRole('dialog', { name: 'Thread actions: Fix login' })).toBeNull();
  expect(within(window).getByRole('radio', { name: /This workspace/ })).toBeTruthy();
  expect(within(window).getByRole('radio', { name: /Whole device/ })).toBeTruthy();
  fireEvent.change(within(window).getByPlaceholderText('username or email'), { target: { value: 'carol' } });
  fireEvent.click(within(window).getByRole('button', { name: 'Share this thread' }));
  await waitFor(() => expect(createRelayShare).toHaveBeenCalledWith({
    targetIdentifier: 'carol', deviceId: 'dev-1', threadId: 't-1', threadTitle: 'Fix login',
    workspaceId: null, workspaceLabel: null, label: null, threadAccess: 'read', workspaceAccess: 'none',
  }));
  // The list reloads after the change.
  await waitFor(() => expect(fetchRelayPortal).toHaveBeenCalledTimes(2));
  fireEvent.click(within(window).getByRole('button', { name: 'Close' }));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Thread actions' })).toBeNull());
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Actions for Fix login' }));
});

it('shares the thread\'s workspace as a workspace grant and revokes a share', async () => {
  vi.mocked(fetchRelayPortal).mockResolvedValue(portal({ sharedByMe: [share] } as never));
  mount();
  await openMenu();
  fireEvent.click(screen.getByRole('button', { name: 'Share thread' }));
  const window = await screen.findByRole('dialog', { name: 'Thread actions' });
  await within(window).findByText('bob');
  fireEvent.click(within(window).getByRole('radio', { name: /This workspace/ }));
  fireEvent.change(within(window).getByPlaceholderText('username or email'), { target: { value: 'dave' } });
  fireEvent.click(within(window).getByRole('button', { name: 'Share workspace' }));
  await waitFor(() => expect(createRelayGrant).toHaveBeenCalledWith(expect.objectContaining({
    targetIdentifier: 'dave', deviceId: 'dev-1', scope: 'workspace', workspaceId: 'ws-1', workspaceLabel: 'App',
  })));
  fireEvent.click(await within(window).findByRole('button', { name: 'Revoke' }));
  await waitFor(() => expect(revokeRelayShare).toHaveBeenCalledWith('share-1'));
});

it('keeps a share\'s thread title current when it lists the shares', async () => {
  vi.mocked(fetchRelayPortal).mockResolvedValue(portal({ sharedByMe: [{ ...share, threadTitle: 'Old title' }] } as never));
  mount();
  await openMenu();
  fireEvent.click(screen.getByRole('button', { name: 'Share thread' }));
  await waitFor(() => expect(updateRelayShare).toHaveBeenCalledWith('share-1', { threadTitle: 'Fix login' }));
});

it('stays open from a tab\'s right-click menu until its share window closes', async () => {
  const onClose = vi.fn();
  const returnFocus = document.createElement('button');
  document.body.append(returnFocus);
  render(<HostCapabilitiesProvider scope="dev-1"><RecentThreadMenu thread={relayThread} currentKey="" context={{ x: 40, y: 40, returnFocus, onClose }} onFavorite={vi.fn()} onRenamed={vi.fn()} onRemoved={vi.fn()} onNavigate={vi.fn()} /></HostCapabilitiesProvider>);
  fireEvent.click(await screen.findByRole('button', { name: 'Share thread' }));
  const window = await screen.findByRole('dialog', { name: 'Thread actions' });
  expect(onClose).not.toHaveBeenCalled();
  fireEvent.click(within(window).getByRole('button', { name: 'Close' }));
  await waitFor(() => expect(onClose).toHaveBeenCalled());
  expect(document.activeElement).toBe(returnFocus);
  returnFocus.remove();
});
