import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { HarnessResourceSettings } from './HarnessResourceSettings';
import { parseResourceField } from './harnessResourceModel';
import {
  ApiError, compactStorage, fetchHarnessResourceSettings, fetchStorageOverview, fetchStorageSettings, pruneStorage,
  relayModeActive, scanStorage, updateHarnessResourceSettings, updateStorageSettings,
} from '../lib/api';

vi.mock('../lib/api', () => {
  class ApiError extends Error {
    constructor(public statusCode: number, public payload: { message: string }) {
      super(payload.message);
    }
  }
  return {
    ApiError, fetchHarnessResourceSettings: vi.fn(), updateHarnessResourceSettings: vi.fn(), relayModeActive: vi.fn(),
    fetchStorageOverview: vi.fn(), fetchStorageSettings: vi.fn(), updateStorageSettings: vi.fn(), scanStorage: vi.fn(),
    pruneStorage: vi.fn(), compactStorage: vi.fn(),
  };
});

const GIB = 2 ** 30;
beforeEach(() => {
  vi.mocked(relayModeActive).mockReset().mockReturnValue(false);
  vi.mocked(fetchHarnessResourceSettings).mockReset().mockResolvedValue({ idleReleaseMinutes: 10, maxLoadedThreads: null, maxHarnessMemoryBytes: null });
  vi.mocked(updateHarnessResourceSettings).mockReset().mockImplementation(async input => input);
  vi.mocked(fetchStorageOverview).mockReset().mockResolvedValue({ deviceName: 'ubuntu-ms', scan: null });
  vi.mocked(fetchStorageSettings).mockReset().mockResolvedValue({ commandOutputRetentionDays: null });
  vi.mocked(updateStorageSettings).mockReset().mockImplementation(async input => input);
});
const memory = () => within(screen.getByRole('region', { name: 'Memory' }));
const renderAt = (path = '/threads/t') => render(<MemoryRouter initialEntries={[path]}><HarnessResourceSettings /></MemoryRouter>);
const field = (name: string) => screen.getByRole('spinbutton', { name }) as HTMLInputElement;
const toggle = (name: string) => screen.getByRole('checkbox', { name }) as HTMLInputElement;

it('reads the settings and saves limits in bytes', async () => {
  renderAt();
  await waitFor(() => expect(field('Release idle threads')).toHaveValue(10));
  expect(fetchHarnessResourceSettings).toHaveBeenCalledWith(null);
  expect(toggle('Limit loaded threads')).not.toBeChecked();
  expect(field('Limit loaded threads')).toBeDisabled();
  fireEvent.click(toggle('Limit loaded threads'));
  fireEvent.change(field('Limit loaded threads'), { target: { value: '6' } });
  fireEvent.click(toggle('Limit harness memory'));
  fireEvent.change(field('Limit harness memory'), { target: { value: '2.5' } });
  fireEvent.click(toggle('Release idle threads'));
  fireEvent.click(memory().getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(updateHarnessResourceSettings).toHaveBeenCalledWith({
    idleReleaseMinutes: null, maxLoadedThreads: 6, maxHarnessMemoryBytes: 2.5 * GIB,
  }, null));
  expect(await screen.findByRole('status')).toHaveTextContent('Saved.');
});

it('rejects values the Supervisor would reject', async () => {
  renderAt();
  await waitFor(() => expect(field('Release idle threads')).toHaveValue(10));
  const save = memory().getByRole('button', { name: 'Save' });
  fireEvent.change(field('Release idle threads'), { target: { value: '1441' } });
  expect(screen.getByText('Enter 1–1440 minutes.')).toBeVisible();
  expect(save).toBeDisabled();
  fireEvent.change(field('Release idle threads'), { target: { value: '1440' } });
  expect(save).toBeEnabled();
  fireEvent.click(toggle('Limit harness memory'));
  fireEvent.change(field('Limit harness memory'), { target: { value: '0.2' } });
  expect(screen.getByText('Enter at least 0.25 GiB.')).toBeVisible();
  expect(save).toBeDisabled();
  fireEvent.click(save);
  expect(updateHarnessResourceSettings).not.toHaveBeenCalled();
});

it('validates each range like the Supervisor', () => {
  expect(parseResourceField('idleReleaseMinutes', true, '0')).toBeUndefined();
  expect(parseResourceField('idleReleaseMinutes', true, '1.5')).toBeUndefined();
  expect(parseResourceField('idleReleaseMinutes', true, '')).toBeUndefined();
  expect(parseResourceField('idleReleaseMinutes', false, 'x')).toBeNull();
  expect(parseResourceField('maxLoadedThreads', true, '200')).toBe(200);
  expect(parseResourceField('maxLoadedThreads', true, '201')).toBeUndefined();
  expect(parseResourceField('maxHarnessMemoryBytes', true, '0.25')).toBe(256 * 2 ** 20);
  // An untouched limit keeps its exact bytes.
  expect(parseResourceField('maxHarnessMemoryBytes', true, '0.29', 300 * 2 ** 20)).toBe(300 * 2 ** 20);
});

it('targets the routed relay device and asks for one without it', async () => {
  vi.mocked(relayModeActive).mockReturnValue(true);
  const { unmount } = renderAt('/workspaces');
  expect(screen.getByText('Open a device to manage its resources.')).toBeVisible();
  expect(fetchHarnessResourceSettings).not.toHaveBeenCalled();
  unmount();
  renderAt('/devices/dev-1/threads/t');
  await waitFor(() => expect(fetchHarnessResourceSettings).toHaveBeenCalledWith('dev-1'));
});

it('explains an older Supervisor instead of failing', async () => {
  vi.mocked(fetchHarnessResourceSettings).mockRejectedValue(new ApiError(404, { code: 'not_found', message: 'Not found' } as never));
  renderAt();
  expect(await screen.findByText("Update this device's Supervisor to change resource settings.")).toBeVisible();
  expect(screen.queryByRole('alert')).toBeNull();
});

const MB = 2 ** 20;
const conversation = (id: string, bytes: number, commandBytes: number) => ({
  threadId: id, title: `Thread ${id}`, workspaceLabel: 'lab', bytes, commandBytes, items: 10, agentThreads: id === 'a' ? 2 : 0, lastActivityAt: null,
});
const scanResult = (offset: number, ids: string[], nextOffset: number | null) => ({
  scannedAt: '2026-10-18T10:00:00.000Z', deviceName: 'ubuntu-ms', databaseBytes: 640 * MB, freeBytes: 2 * MB,
  historyBytes: 450 * MB, commandBytes: 373 * MB, prunedItems: 0, prunedBytes: 0, threadCount: 300, conversationCount: 74,
  threads: ids.map((id, index) => conversation(id, (60 - index) * MB, (50 - index) * MB)), offset, nextOffset,
});

it('measures the disk only on request and pages the largest conversations', async () => {
  vi.mocked(scanStorage).mockResolvedValue(scanResult(0, ['a', 'b'], 2));
  vi.mocked(fetchStorageOverview).mockResolvedValueOnce({ deviceName: 'ubuntu-ms', scan: null })
    .mockResolvedValueOnce({ deviceName: 'ubuntu-ms', scan: scanResult(2, ['c'], null) });
  renderAt();
  const disk = within(await screen.findByRole('region', { name: 'Disk' }));
  expect(await disk.findByText('ubuntu-ms')).toBeVisible();
  expect(scanStorage).not.toHaveBeenCalled();
  fireEvent.click(disk.getByRole('button', { name: 'Scan' }));
  expect(await disk.findByText('Thread a')).toBeVisible();
  expect(disk.getByText('640 MB')).toBeVisible();
  expect(disk.getByText('83% of history')).toBeVisible();
  expect(disk.getByText('lab · with 2 agent threads')).toBeVisible();
  expect(disk.getByText('Commands 50 MB · 83%')).toBeVisible();
  fireEvent.click(disk.getByRole('button', { name: 'Load more' }));
  expect(await disk.findByText('Thread c')).toBeVisible();
  expect(fetchStorageOverview).toHaveBeenLastCalledWith(2, null);
  expect(disk.queryByRole('button', { name: 'Load more' })).toBeNull();
});

it('saves a command-output retention and cleans up only after a red confirmation', async () => {
  vi.mocked(pruneStorage).mockResolvedValue({ items: 12, bytes: 3 * MB });
  renderAt();
  const disk = within(await screen.findByRole('region', { name: 'Disk' }));
  const clean = await disk.findByRole('button', { name: 'Clean up now' });
  expect(clean).toBeDisabled();
  fireEvent.click(disk.getByRole('checkbox', { name: 'Clean up command output' }));
  fireEvent.change(disk.getByRole('spinbutton', { name: 'Clean up command output' }), { target: { value: '14' } });
  fireEvent.click(disk.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(updateStorageSettings).toHaveBeenCalledWith({ commandOutputRetentionDays: 14 }, null));
  await waitFor(() => expect(clean).toBeEnabled());
  fireEvent.click(clean);
  const dialog = await screen.findByRole('dialog');
  expect(dialog).toHaveTextContent('over 14 days ago');
  expect(pruneStorage).not.toHaveBeenCalled();
  fireEvent.click(within(dialog).getByRole('button', { name: 'Clean up now' }));
  expect(await disk.findByText('Cleaned up 12 outputs, 3 MB')).toBeVisible();
  expect(compactStorage).not.toHaveBeenCalled();
});
