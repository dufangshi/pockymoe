import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { HarnessResourceSettings } from './HarnessResourceSettings';
import { parseResourceField } from './harnessResourceModel';
import {
  ApiError, cleanStorageThread, compactStorage, fetchHarnessResourceSettings, fetchStorageOverview, fetchStorageSettings, pruneStorage,
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
    pruneStorage: vi.fn(), compactStorage: vi.fn(), cleanStorageThread: vi.fn(),
  };
});

const GIB = 2 ** 30;
const DEFAULT_CLEANUP = { commandOutput: true, commandText: 'keep' as const, commandTextChars: 512, fileDiffs: false };
beforeEach(() => {
  vi.mocked(relayModeActive).mockReset().mockReturnValue(false);
  vi.mocked(fetchHarnessResourceSettings).mockReset().mockResolvedValue({ idleReleaseMinutes: 10, maxLoadedThreads: null, maxHarnessMemoryBytes: null });
  vi.mocked(updateHarnessResourceSettings).mockReset().mockImplementation(async input => input);
  vi.mocked(fetchStorageOverview).mockReset().mockResolvedValue({ deviceName: 'ubuntu-ms', scan: null });
  vi.mocked(fetchStorageSettings).mockReset().mockResolvedValue({ retentionDays: null, ...DEFAULT_CLEANUP });
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
  threadId: id, title: `Thread ${id}`, workspaceLabel: 'lab', bytes, commandBytes, fileChangeBytes: 5 * MB, items: 10, agentThreads: id === 'a' ? 2 : 0, lastActivityAt: null,
});
const scanResult = (offset: number, ids: string[], nextOffset: number | null) => ({
  scannedAt: '2026-10-18T10:00:00.000Z', deviceName: 'ubuntu-ms', databaseBytes: 640 * MB, freeBytes: 2 * MB,
  historyBytes: 450 * MB, commandBytes: 373 * MB, fileChangeBytes: 45 * MB, prunedItems: 0, prunedBytes: 0, threadCount: 300, conversationCount: 74,
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
  expect(disk.getByText('10% of history')).toBeVisible();
  expect(disk.getByText('Commands 50 MB · diffs 5 MB')).toBeVisible();
  fireEvent.click(disk.getByRole('button', { name: 'Load more' }));
  expect(await disk.findByText('Thread c')).toBeVisible();
  expect(fetchStorageOverview).toHaveBeenLastCalledWith(2, null);
  expect(disk.queryByRole('button', { name: 'Load more' })).toBeNull();
});

const job = (state: 'running' | 'done', processed: number, items: number, bytes: number) => ({
  action: 'prune' as const, state, total: 1000, processed, items, bytes, beforeBytes: null, afterBytes: null,
  startedAt: '2026-10-19T00:00:00.000Z', finishedAt: state === 'done' ? '2026-10-19T00:01:00.000Z' : null, error: null,
});

it('saves one age with what to clean up, then cleans up in the background with progress after a red confirmation', async () => {
  // Just started: the device is still counting what to clean up.
  vi.mocked(pruneStorage).mockResolvedValue({ ...job('running', 0, 0, 0), total: 0 });
  renderAt();
  const disk = within(await screen.findByRole('region', { name: 'Disk' }));
  const clean = await disk.findByRole('button', { name: 'Clean up now' });
  expect(clean).toBeDisabled();
  fireEvent.click(disk.getByRole('checkbox', { name: 'Clean up automatically' }));
  fireEvent.change(disk.getByRole('spinbutton', { name: 'Clean up automatically' }), { target: { value: '14' } });
  // Nothing chosen cannot be saved.
  fireEvent.click(disk.getByRole('checkbox', { name: 'Command output' }));
  expect(disk.getByText('Choose something to clean up.')).toBeVisible();
  expect(disk.getByRole('button', { name: 'Save' })).toBeDisabled();
  fireEvent.click(disk.getByRole('checkbox', { name: 'Command output' }));
  fireEvent.change(disk.getByRole('combobox', { name: 'Command text' }), { target: { value: 'truncate' } });
  fireEvent.change(disk.getByRole('spinbutton', { name: 'Keep the first' }), { target: { value: '200' } });
  fireEvent.click(disk.getByRole('checkbox', { name: 'File-change diffs' }));
  fireEvent.click(disk.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(updateStorageSettings).toHaveBeenCalledWith(
    { retentionDays: 14, commandOutput: true, commandText: 'truncate', commandTextChars: 200, fileDiffs: true }, null));
  await waitFor(() => expect(clean).toBeEnabled());
  fireEvent.click(clean);
  const dialog = await screen.findByRole('dialog');
  expect(dialog).toHaveTextContent('Removes command output, command text past 200 characters, file-change diffs from turns that ended over 14 days ago.');
  expect(pruneStorage).not.toHaveBeenCalled();
  vi.mocked(fetchStorageOverview).mockResolvedValueOnce({ deviceName: 'ubuntu-ms', scan: null, job: job('running', 400, 380, 2 * MB) })
    .mockResolvedValue({ deviceName: 'ubuntu-ms', scan: null, job: job('done', 1000, 912, 3 * MB) });
  fireEvent.click(within(dialog).getByRole('button', { name: 'Clean up now' }));
  // The dialog closes at once; the job's progress follows on the page.
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  await waitFor(() => expect(pruneStorage).toHaveBeenCalled());
  expect(await disk.findByText('Finding what to clean up…')).toBeVisible();
  expect(await disk.findByText('Cleaning up 400 / 1,000 · 2 MB removed', {}, { timeout: 3000 })).toBeVisible();
  expect(clean).toBeDisabled();
  expect(await disk.findByText('Cleaned up 912 items, 3 MB', {}, { timeout: 3000 })).toBeVisible();
  expect(compactStorage).not.toHaveBeenCalled();
});

it('cleans up one conversation from its menu, whatever its age, with progress on its row', async () => {
  vi.mocked(scanStorage).mockResolvedValue(scanResult(0, ['a', 'b'], null));
  vi.mocked(cleanStorageThread).mockResolvedValue({ ...job('running', 0, 0, 0), total: 0, threadId: 'a' });
  renderAt();
  const disk = within(await screen.findByRole('region', { name: 'Disk' }));
  fireEvent.click(await disk.findByRole('button', { name: 'Scan' }));
  fireEvent.click(await disk.findByRole('button', { name: 'Clean up “Thread a”' }));
  const menu = await screen.findByRole('menu', { name: 'Clean up “Thread a”' });
  expect(within(menu).getAllByRole('menuitem').map(item => item.textContent)).toEqual([
    'Clean up command output', 'Clean up diffs', 'Shorten commands to 512 characters', 'Remove commands',
  ]);
  await waitFor(() => expect(within(menu).getAllByRole('menuitem')[0]).toHaveFocus());
  fireEvent.click(within(menu).getByRole('menuitem', { name: 'Remove commands' }));
  expect(screen.queryByRole('menu')).toBeNull();
  const dialog = await screen.findByRole('dialog');
  expect(dialog).toHaveTextContent('Clean up “Thread a”?');
  expect(dialog).toHaveTextContent('Removes command text from every ended turn of this conversation and its agent threads, however recent.');
  vi.mocked(fetchStorageOverview).mockResolvedValue({ deviceName: 'ubuntu-ms', scan: null, job: { ...job('running', 400, 380, MB), threadId: 'a' } });
  fireEvent.click(within(dialog).getByRole('button', { name: 'Clean up now' }));
  await waitFor(() => expect(cleanStorageThread).toHaveBeenCalledWith('a', { commandOutput: false, commandText: 'remove', commandTextChars: 512, fileDiffs: false }, null));
  expect(await disk.findByText('Cleaning up 400 / 1,000', {}, { timeout: 3000 })).toBeVisible();
  expect(disk.getByRole('button', { name: 'Clean up “Thread b”' })).toBeEnabled();
  expect(disk.getByRole('button', { name: 'Clean up now' })).toBeDisabled();
});
