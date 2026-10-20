import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import type { RuntimeConfigDto } from '@pockymoe/shared';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { fetchRuntimeConfig, fetchThreads, fetchWorkspaces } from '../lib/api';
import { HostCapabilitiesProvider, UnscopedOnly } from '../lib/hostCapabilities';
import { HostScopeRouteGate } from './HostScopeRouteGate';
import { ThreadsPage } from '../pages/ThreadsPage';
import { ThreadWorkspaceLayout } from './ThreadWorkspaceLayout';

vi.mock('../lib/api', async original => ({ ...(await original<typeof import('../lib/api')>()), fetchRuntimeConfig: vi.fn(), fetchThreads: vi.fn(), fetchWorkspaces: vi.fn() }));
vi.mock('./DeviceMonitor', () => ({ DeviceMonitor: () => <button>Device monitor</button> }));
vi.mock('./onboarding/TourProvider', () => ({ TourLauncherButton: () => <button>Tour</button> }));
const scoped = {
  hostScope: { kind: 'treerAgent', boundThreadId: 'root', hubId: 'hub', allowed: {
    boundThreads: true, childThreads: true, topLevelThreads: true,
    workspaceCreation: false, management: false, workspaceFiles: 'read',
  } },
} as RuntimeConfigDto;
beforeEach(() => { Element.prototype.scrollIntoView = vi.fn(); setLocale('en', false); vi.mocked(fetchRuntimeConfig).mockResolvedValue(scoped); });
afterEach(() => { cleanup(); vi.clearAllMocks(); });

it.each(['/threads/import', '/workspaces/new', '/workspaces', '/relay-settings', '/relay-devices'])('redirects scoped %s before mounting its page', async path => {
  const denied = vi.fn();
  function DeniedPage() { denied(); return <p>Denied page</p>; }
  render(<MemoryRouter initialEntries={[path]}><HostCapabilitiesProvider scope="local"><Routes>
    <Route path={path} element={<HostScopeRouteGate><DeniedPage /></HostScopeRouteGate>} />
    <Route path="/threads/root" element={<p>Bound thread</p>} />
  </Routes></HostCapabilitiesProvider></MemoryRouter>);
  expect(await screen.findByText('Bound thread')).toBeVisible();
  expect(screen.queryByText('Denied page')).toBeNull();
  expect(denied).not.toHaveBeenCalled();
});

it('keeps child routes and allowed controls, keeps creation available and hides global controls', async () => {
  render(<MemoryRouter initialEntries={['/threads/child']}><HostCapabilitiesProvider scope="local">
    <HostScopeRouteGate><ThreadWorkspaceLayout threads={[]} status={null}
      workbench={{ threads: [{ key: 'local:child', title: 'Child', href: '/threads/child', subtitle: 'Child thread', status: 'idle', favorite: false }], currentKey: 'local:child', activeView: 'chat', terminalEnabled: false, onViewChange: () => {}, onSearch: () => {}, favorite: false,
        workspacePath: '/project', onToggleFavorite: () => {}, onNavigate: () => {},
        notifications: [], unreadCount: 0, onReadNotifications: () => {} }}>
      <button>Delegate thread</button><UnscopedOnly><button>Management</button></UnscopedOnly>
    </ThreadWorkspaceLayout></HostScopeRouteGate>
  </HostCapabilitiesProvider></MemoryRouter>);
  expect(await screen.findByRole('button', { name: 'Delegate thread' })).toBeVisible();
  expect(screen.getByRole('button', { name: /New chat/i })).toBeVisible();
  for (const label of ['Open settings', 'Device monitor', 'Management']) expect(screen.queryByRole('button', { name: label })).toBeNull();
  expect(document.querySelector('a[href="/workspaces"]')).toBeNull();
  expect(document.querySelector('a[href="/threads/child"]')).not.toBeNull();
});

it('does not mount unrestricted controls while scope is loading or discovery fails', async () => {
  vi.mocked(fetchRuntimeConfig).mockRejectedValue(new Error('offline'));
  render(<HostCapabilitiesProvider scope="local"><button>Management</button></HostCapabilitiesProvider>);
  await waitFor(() => expect(screen.getByRole('alert')).toBeVisible());
  expect(screen.queryByRole('button', { name: 'Management' })).toBeNull();
});

it.each(['/threads/new', '/threads', '/threads/another-bound-root'])('allows scoped workbench navigation to %s', async path => {
  render(<MemoryRouter initialEntries={[path]}><HostCapabilitiesProvider scope="local"><HostScopeRouteGate><p>Allowed page</p></HostScopeRouteGate></HostCapabilitiesProvider></MemoryRouter>);
  expect(await screen.findByText('Allowed page')).toBeVisible();
});

it('lets the reader choose another bound Agent from the unselected scoped workspace', async () => {
  vi.mocked(fetchThreads).mockResolvedValue([{ id:'333', title:'Codex conversation',workspaceId:'project',updatedAt:'2026-10-14T01:00:00Z' }] as never);
  vi.mocked(fetchWorkspaces).mockResolvedValue([{id:'project',label:'Project',absPath:'/project'}] as never);
  render(<MemoryRouter initialEntries={['/threads']}><HostCapabilitiesProvider scope="local"><Routes>
    <Route path="/threads" element={<HostScopeRouteGate><ThreadsPage /></HostScopeRouteGate>} />
    <Route path="/threads/333" element={<p>Codex conversation opened</p>} />
  </Routes></HostCapabilitiesProvider></MemoryRouter>);
  expect(await screen.findByRole('heading', { name: 'Project', exact: true })).toBeVisible();
  expect(screen.queryByText('Codex conversation opened')).toBeNull();
  fireEvent.click(screen.getByTestId('workspace-threads').querySelector('a')!);
  expect(await screen.findByText('Codex conversation opened')).toBeVisible();
  expect(fetchThreads).toHaveBeenCalledWith(true);
  expect(screen.queryByRole('alert')).toBeNull();
});
