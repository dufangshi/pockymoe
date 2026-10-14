import { HostCapabilitiesProvider } from '../lib/hostCapabilities';
import type { RuntimeConfigDto } from '@pockymoe/shared';
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ThreadDto, WorkspaceDto } from '@pockymoe/shared';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { request, fetchRuntimeConfig } from '../lib/api';
import { WorkbenchThreadPicker } from './WorkbenchThreadPicker';
vi.mock('../lib/api', () => ({ request: vi.fn(), fetchRuntimeConfig: vi.fn(), relayModeActive: () => false }));
const thread = (id: string, workspaceId = 'current'): ThreadDto =>
  ({
    id,
    workspaceId,
    title: id,
    status: 'idle',
    updatedAt: '2026-10-08',
  }) as ThreadDto;
const workspace = (id: string): WorkspaceDto =>
  ({ id, label: id, absPath: `/code/${id}` }) as WorkspaceDto;
beforeEach(() => {
  setLocale('en', false);
  vi.mocked(request).mockReset();
});

describe('hierarchical split picker', () => {
  it('shows current peers first and lazily selects a full remote device/workspace/thread identity', async () => {
    const peers = [
      thread('host-thread'),
      thread('recent'),
      thread('favorite'),
      thread('elsewhere', 'other-workspace'),
    ];
    vi.mocked(request).mockImplementation(async (path) => {
      if (path === 'relay/portal')
        return {
          devices: [
            { id: 'host', name: 'Host' },
            { id: 'remote', name: 'Second device', connected: true },
          ],
        };
      if (path === 'relay/devices/host/api/workspaces')
        return [workspace('current'), workspace('other-workspace')];
      if (path === 'relay/devices/host/api/threads?includeAgentThreads=true')
        return peers;
      if (path === 'relay/devices/remote/api/workspaces')
        return [workspace('remote-workspace')];
      if (path === 'relay/devices/remote/api/threads?includeAgentThreads=true')
        return [thread('remote-peer', 'remote-workspace')];
      throw new Error(`Unexpected request: ${path}`);
    });
    const selected = vi.fn();
    render(
      <WorkbenchThreadPicker
        deviceId="host"
        workspaceId="current"
        threadId="host-thread"
        threads={peers}
        navigationThreads={[
          { key: 'host:recent', favorite: false },
          { key: 'host:favorite', favorite: true },
        ]}
        onSelect={selected}
      />,
    );
    expect(request).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Split conversation' }));
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
    const dialog = screen.getByRole('dialog');
    const labels = within(dialog)
      .getAllByRole('button')
      .map((button) => button.textContent);
    expect(labels.indexOf('favoriteReady')).toBeLessThan(
      labels.indexOf('recentReady'),
    );
    expect(
      within(dialog).queryByRole('button', { name: /host-thread/ }),
    ).not.toBeInTheDocument();
    await screen.findByRole('button', { name: /Second device/ });
    expect(
      vi
        .mocked(request)
        .mock.calls.some(([path]) => String(path).includes('/remote/')),
    ).toBe(false);
    fireEvent.click(screen.getByRole('button', { name: /Second device/ }));
    await screen.findByRole('button', { name: /remote-workspace.*code/ });
    expect(
      vi
        .mocked(request)
        .mock.calls.some(([path]) =>
          String(path).includes('/remote/api/threads'),
        ),
    ).toBe(false);
    fireEvent.click(
      screen.getByRole('button', { name: /remote-workspace.*code/ }),
    );
    fireEvent.click(await screen.findByRole('button', { name: /remote-peer/ }));
    expect(selected).toHaveBeenCalledWith({
      deviceId: 'remote',
      workspaceId: 'remote-workspace',
      threadId: 'remote-peer',
      title: 'remote-peer',
    });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: 'Split conversation' }),
    ).toHaveFocus();
  });
  it('supports local workspaces, retry, keyboard back and Escape without changing the selected route', async () => {
    let denied = true;
    vi.mocked(request).mockImplementation(async (path) => {
      if (path === 'api/workspaces') {
        if (denied) throw new Error('Access denied');
        return [workspace('current'), workspace('other')];
      }
      if (path === 'api/threads?includeAgentThreads=true')
        return [thread('local-peer', 'other')];
      throw new Error(`Unexpected request: ${path}`);
    });
    const selected = vi.fn();
    const initialHref = window.location.href;
    render(
      <WorkbenchThreadPicker
        deviceId={null}
        workspaceId="current"
        threadId="host-thread"
        threads={[]}
        onSelect={selected}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Split conversation' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Access denied');
    denied = false;
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    fireEvent.click(await screen.findByRole('button', { name: /other.*code/ }));
    await screen.findByRole('button', { name: /local-peer/ });
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'ArrowLeft' });
    expect(screen.getByText('THIS WORKSPACE')).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'End' });
    expect(screen.getByRole('button', { name: /other.*code/ })).toHaveFocus();
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(window.location.href).toBe(initialHref);
    expect(selected).not.toHaveBeenCalled();
    expect(
      vi
        .mocked(request)
        .mock.calls.every(([path]) => String(path).startsWith('api/')),
    ).toBe(true);
  });
  it('keeps one toolbar trigger and places restore and close inside its menu', async () => {
    vi.mocked(request).mockResolvedValue([]);
    const restore = vi.fn(),
      close = vi.fn();
    const props = {
      deviceId: null,
      workspaceId: 'current',
      threadId: 'host',
      threads: [],
      onSelect: vi.fn(),
      reference: { deviceId: null, threadId: 'peer' },
      onRestore: restore,
      onClose: close,
    };
    const { rerender } = render(
      <WorkbenchThreadPicker {...props} splitActive={false} />,
    );
    expect(document.querySelector('.workbench-thread-picker-actions')!.querySelectorAll('button')).toHaveLength(1);
    expect(screen.queryByRole('button', { name: 'Restore split' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Split conversation' }));
    fireEvent.click(screen.getByRole('button', { name: 'Restore split' }));
    expect(restore).toHaveBeenCalledOnce();
    rerender(<WorkbenchThreadPicker {...props} splitActive />);
    expect(document.querySelector('.workbench-thread-picker-actions')!.querySelectorAll('button')).toHaveLength(1);
    expect(screen.queryByRole('button', { name: 'Close split' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Split conversation' }));
    fireEvent.click(screen.getByRole('button', { name: 'Close split' }));
    expect(close).toHaveBeenCalledOnce();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
});

it('keeps scoped split selection across bound workspaces and hides devices', async () => {
  vi.mocked(fetchRuntimeConfig).mockResolvedValue({ hostScope: { kind: 'treerAgent', boundThreadId: 'root' } } as RuntimeConfigDto);
  const peers = [thread('root'), { ...thread('child'), rootThreadId: 'root', parentThreadId: 'root' }];
  vi.mocked(request).mockImplementation(async path => {
    if (path === 'api/workspaces') return [workspace('current')];
    if (path === 'api/threads?includeAgentThreads=true') return peers;
    throw new Error(`Unexpected scoped request: ${path}`);
  });
  const selected = vi.fn();
  render(<HostCapabilitiesProvider scope="local"><WorkbenchThreadPicker deviceId={null}
    workspaceId="current" threadId="root" threads={peers} onSelect={selected} />
  </HostCapabilitiesProvider>);
  fireEvent.click(await screen.findByRole('button', { name: 'Split conversation' }));
  expect(screen.getByText('OTHER WORKSPACES · THIS DEVICE')).toBeVisible();
  expect(screen.queryByText('OTHER DEVICES')).toBeNull();
  fireEvent.click(await screen.findByRole('button', { name: /child/ }));
  expect(selected).toHaveBeenCalledWith({ deviceId: null, workspaceId: 'current', threadId: 'child', title: 'child' });
  expect(vi.mocked(request).mock.calls.some(([path]) => String(path).startsWith('relay/'))).toBe(false);
});
